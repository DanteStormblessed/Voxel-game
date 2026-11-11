//! Transporte QUIC compartido por cliente y servidor.
//!
//! Cada conexión usa un stream bidireccional para los mensajes fiables (enmarcados con
//! su longitud) y datagramas para los no fiables (posiciones, que caducan enseguida).

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context};
use bytes::Bytes;
use quinn::{Connection, Endpoint, RecvStream, SendStream};
use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use serde::de::DeserializeOwned;
use serde::Serialize;

/// Tamaño máximo de un mensaje fiable. Corta en seco a un par que envíe basura.
const MAX_FRAME: usize = 4 * 1024 * 1024;
const ALPN: &[u8] = b"voxel/1";

fn install_crypto() {
    // Falla si ya había un proveedor instalado, que es justo lo que queremos.
    let _ = rustls::crypto::ring::default_provider().install_default();
}

fn transport() -> anyhow::Result<Arc<quinn::TransportConfig>> {
    let mut transport = quinn::TransportConfig::default();
    transport.keep_alive_interval(Some(Duration::from_secs(4)));
    transport.max_idle_timeout(Some(Duration::from_secs(20).try_into()?));
    Ok(Arc::new(transport))
}

/// Abre un endpoint de servidor con un certificado autofirmado generado al vuelo.
pub fn server_endpoint(addr: SocketAddr) -> anyhow::Result<Endpoint> {
    install_crypto();
    let cert = rcgen::generate_simple_self_signed(vec!["voxel".to_string()])?;
    let cert_der = CertificateDer::from(cert.cert.der().to_vec());
    let key = PrivatePkcs8KeyDer::from(cert.key_pair.serialize_der());

    let mut crypto = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert_der], key.into())?;
    crypto.alpn_protocols = vec![ALPN.to_vec()];
    let mut config = quinn::ServerConfig::with_crypto(Arc::new(
        quinn::crypto::rustls::QuicServerConfig::try_from(crypto)?,
    ));
    config.transport_config(transport()?);
    Endpoint::server(config, addr).with_context(|| format!("failed to listen on {addr}"))
}

/// Conecta con un servidor.
///
/// El cifrado está activo pero **no se comprueba la identidad del servidor** (los
/// certificados son autofirmados). Pendiente: fijar la huella del servidor.
pub async fn connect(addr: SocketAddr) -> anyhow::Result<(Endpoint, Connection)> {
    install_crypto();
    let mut crypto = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AcceptAnyServer::new()))
        .with_no_client_auth();
    crypto.alpn_protocols = vec![ALPN.to_vec()];
    let mut config = quinn::ClientConfig::new(Arc::new(
        quinn::crypto::rustls::QuicClientConfig::try_from(crypto)?,
    ));
    config.transport_config(transport()?);

    let bind: SocketAddr = if addr.is_ipv6() { "[::]:0" } else { "0.0.0.0:0" }.parse()?;
    let mut endpoint = Endpoint::client(bind)?;
    endpoint.set_default_client_config(config);
    let connection = endpoint
        .connect(addr, "voxel")?
        .await
        .with_context(|| format!("failed to connect to {addr}"))?;
    Ok((endpoint, connection))
}

pub fn encode<T: Serialize>(msg: &T) -> anyhow::Result<Vec<u8>> {
    Ok(postcard::to_allocvec(msg)?)
}

/// Escribe un mensaje fiable: longitud (u32 LE) + cuerpo.
pub async fn write_msg<T: Serialize>(stream: &mut SendStream, msg: &T) -> anyhow::Result<()> {
    let body = encode(msg)?;
    if body.len() > MAX_FRAME {
        bail!("message too large: {} bytes", body.len());
    }
    stream.write_all(&(body.len() as u32).to_le_bytes()).await?;
    stream.write_all(&body).await?;
    Ok(())
}

/// Lee un mensaje fiable. `Ok(None)` si el otro extremo cerró el stream limpiamente.
pub async fn read_msg<T: DeserializeOwned>(stream: &mut RecvStream) -> anyhow::Result<Option<T>> {
    let mut len = [0u8; 4];
    match stream.read_exact(&mut len).await {
        Ok(()) => {}
        Err(quinn::ReadExactError::FinishedEarly(0)) => return Ok(None),
        Err(error) => return Err(error.into()),
    }
    let len = u32::from_le_bytes(len) as usize;
    if len > MAX_FRAME {
        bail!("message too large: {len} bytes");
    }
    let mut body = vec![0u8; len];
    stream.read_exact(&mut body).await?;
    Ok(Some(postcard::from_bytes(&body)?))
}

/// Envía un mensaje no fiable. Si no cabe en un datagrama se descarta.
pub fn send_datagram<T: Serialize>(connection: &Connection, msg: &T) -> anyhow::Result<()> {
    let body = encode(msg)?;
    match connection.send_datagram(Bytes::from(body)) {
        Ok(()) | Err(quinn::SendDatagramError::TooLarge) => Ok(()),
        Err(error) => Err(error.into()),
    }
}

pub async fn read_datagram<T: DeserializeOwned>(connection: &Connection) -> anyhow::Result<T> {
    let bytes = connection.read_datagram().await?;
    Ok(postcard::from_bytes(&bytes)?)
}

#[derive(Debug)]
struct AcceptAnyServer(Arc<rustls::crypto::CryptoProvider>);

impl AcceptAnyServer {
    fn new() -> Self {
        Self(Arc::new(rustls::crypto::ring::default_provider()))
    }
}

impl rustls::client::danger::ServerCertVerifier for AcceptAnyServer {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}
