//! Conexión del cliente con un servidor.
//!
//! El juego solo ve mensajes: los envía con `send` y los recoge de `incoming`. Debajo
//! puede haber canales en memoria (servidor integrado) o QUIC (servidor remoto).

use std::net::SocketAddr;
use std::sync::mpsc::{channel, Receiver, Sender};

use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};
use voxel_common::net;
use voxel_common::protocol::{ClientMsg, ServerMsg};
use voxel_server::ServerHandle;

pub struct Connection {
    send: Box<dyn Fn(ClientMsg)>,
    pub incoming: Receiver<ServerMsg>,
}

impl Connection {
    pub fn send(&self, msg: ClientMsg) {
        (self.send)(msg);
    }

    /// Conexión en memoria con un servidor que corre en este mismo proceso: los mensajes
    /// no se serializan y los chunks se comparten sin copiar.
    pub fn local(server: &ServerHandle) -> Self {
        let (tx, incoming) = channel();
        let id = server.connect(Box::new(move |msg| tx.send(msg).is_ok()));
        let server = server.clone();
        Self {
            send: Box::new(move |msg| server.message(id, msg)),
            incoming,
        }
    }

    /// Conexión QUIC con un servidor remoto. Devuelve enseguida; si la conexión falla
    /// llega un `ServerMsg::Disconnect` por `incoming`.
    pub fn remote(addr: SocketAddr) -> anyhow::Result<Self> {
        let (tx, incoming) = channel();
        let (outgoing, queue) = unbounded_channel::<ClientMsg>();
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
        std::thread::Builder::new().name("client-net".into()).spawn(move || {
            let reason = match runtime.block_on(run(addr, queue, tx.clone())) {
                Ok(()) => "connection closed".to_string(),
                Err(error) => format!("{error:#}"),
            };
            let _ = tx.send(ServerMsg::Disconnect { reason });
        })?;
        Ok(Self {
            send: Box::new(move |msg| {
                let _ = outgoing.send(msg);
            }),
            incoming,
        })
    }
}

async fn run(
    addr: SocketAddr,
    mut queue: UnboundedReceiver<ClientMsg>,
    tx: Sender<ServerMsg>,
) -> anyhow::Result<()> {
    let (_endpoint, connection) = net::connect(addr).await?;
    let (mut send, mut recv) = connection.open_bi().await?;
    log::info!("connected to {addr}");

    let reliable = {
        let tx = tx.clone();
        async move {
            while let Some(msg) = net::read_msg::<ServerMsg>(&mut recv).await? {
                if tx.send(msg).is_err() {
                    break;
                }
            }
            anyhow::Ok(())
        }
    };
    let datagrams = {
        let connection = connection.clone();
        async move {
            loop {
                match net::read_datagram::<ServerMsg>(&connection).await {
                    Ok(msg) if msg.is_unreliable() => {
                        if tx.send(msg).is_err() {
                            break;
                        }
                    }
                    Ok(_) => {}
                    Err(_) if connection.close_reason().is_some() => break,
                    Err(_) => {}
                }
            }
            anyhow::Ok(())
        }
    };
    let writer = {
        let connection = connection.clone();
        async move {
            while let Some(msg) = queue.recv().await {
                if msg.is_unreliable() {
                    net::send_datagram(&connection, &msg)?;
                } else {
                    net::write_msg(&mut send, &msg).await?;
                }
            }
            anyhow::Ok(())
        }
    };

    let result = tokio::select! {
        result = reliable => result,
        result = datagrams => result,
        result = writer => result,
    };
    connection.close(0u32.into(), b"done");
    result
}
