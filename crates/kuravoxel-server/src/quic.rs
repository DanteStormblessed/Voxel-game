//! Escucha de conexiones QUIC para el servidor.

use std::net::SocketAddr;

use tokio::sync::mpsc::unbounded_channel;
use kuravoxel_common::net;
use kuravoxel_common::protocol::{ClientMsg, ServerMsg};

use crate::ServerHandle;

/// Abre el servidor a la red en un hilo propio. Devuelve la dirección en la que escucha.
pub fn listen(handle: ServerHandle, addr: SocketAddr) -> anyhow::Result<SocketAddr> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("server-net")
        .enable_all()
        .build()?;
    let endpoint = {
        let _guard = runtime.enter();
        net::server_endpoint(addr)?
    };
    let local = endpoint.local_addr()?;
    log::info!("listening on {local} (QUIC/UDP)");

    std::thread::Builder::new().name("server-accept".into()).spawn(move || {
        runtime.block_on(async move {
            while let Some(incoming) = endpoint.accept().await {
                let handle = handle.clone();
                tokio::spawn(async move {
                    match incoming.await {
                        Ok(connection) => serve(handle, connection).await,
                        Err(error) => log::warn!("incoming connection failed: {error}"),
                    }
                });
            }
        });
    })?;
    Ok(local)
}

async fn serve(handle: ServerHandle, connection: quinn::Connection) {
    let remote = connection.remote_address();
    let (mut send, mut recv) = match connection.accept_bi().await {
        Ok(streams) => streams,
        Err(error) => {
            log::warn!("{remote}: did not open the control stream: {error}");
            return;
        }
    };
    log::info!("{remote}: connected");

    let (outgoing, mut queue) = unbounded_channel::<ServerMsg>();
    let id = handle.connect(Box::new(move |msg| outgoing.send(msg).is_ok()));

    // Salida: mensajes del servidor hacia el cliente.
    let writer = {
        let connection = connection.clone();
        tokio::spawn(async move {
            while let Some(msg) = queue.recv().await {
                let closing = matches!(msg, ServerMsg::Disconnect { .. });
                let result = if msg.is_unreliable() {
                    net::send_datagram(&connection, &msg)
                } else {
                    net::write_msg(&mut send, &msg).await
                };
                if result.is_err() {
                    break;
                }
                if closing {
                    let _ = send.finish();
                    let _ = send.stopped().await;
                    break;
                }
            }
            connection.close(0u32.into(), b"done");
        })
    };

    // Entrada: datagramas (movimiento) y stream fiable.
    let datagrams = {
        let (handle, connection) = (handle.clone(), connection.clone());
        tokio::spawn(async move {
            loop {
                match net::read_datagram::<ClientMsg>(&connection).await {
                    Ok(msg) if msg.is_unreliable() => handle.message(id, msg),
                    // Un mensaje fiable por datagrama o basura: se ignora.
                    Ok(_) => {}
                    Err(_) if connection.close_reason().is_some() => break,
                    Err(_) => {}
                }
            }
        })
    };
    loop {
        match net::read_msg::<ClientMsg>(&mut recv).await {
            Ok(Some(msg)) => handle.message(id, msg),
            Ok(None) => break,
            Err(error) => {
                if connection.close_reason().is_none() {
                    log::warn!("{remote}: protocol error: {error:#}");
                }
                break;
            }
        }
    }

    handle.disconnect(id);
    datagrams.abort();
    connection.close(0u32.into(), b"done");
    let _ = writer.await;
    log::info!("{remote}: disconnected");
}
