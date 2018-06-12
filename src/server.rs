//! A websocket chat server.


use std::str;
use std::sync::{Arc, Mutex};
use std::net::{SocketAddr};
use std::thread::{self, JoinHandle};
use ws::{self, Sender as WsSender, WebSocket, Message, Handler, Handshake, CloseCode, Factory};
use bincode;
use chrono::Utc;
use ::{UiRemote, Pingstamp};


struct ServerHandlerInner {
    ui_remote: UiRemote,
}


/// A chat server handler.
struct ServerHandler {
	inner: Arc<Mutex<ServerHandlerInner>>,
    output: WsSender,
}

impl Handler for ServerHandler {
    fn on_shutdown(&mut self) {  }

    fn on_open(&mut self, shake: Handshake) -> Result<(), ws::Error> {
        self.inner.lock().unwrap().ui_remote.server_connected(shake);
        Ok(())
    }

    fn on_message(&mut self, msg: Message) -> Result<(), ws::Error> {
        match msg {
            Message::Text(s) => {
                self.inner.lock().unwrap().ui_remote.message_recvd(s);
                Ok(())
            },
            Message::Binary(b) => {
                match bincode::deserialize::<Pingstamp>(&b) {
                    Ok(Pingstamp::Ping(ts)) => {
                        self.output.send(bincode::serialize(&Pingstamp::Pong(ts)).unwrap())
                    },
                    Ok(Pingstamp::Pong(ts)) => {
                    	let elapsed = Utc::now().signed_duration_since(ts);
                        self.inner.lock().unwrap().ui_remote.pong_recvd(elapsed);
                        Ok(())
                    }
                    Err(_err) => {
                        // self.inner.lock().unwrap().ui_remote.client_error(err.into());
                        Ok(())
                    },
                }
            },
        }
    }

    fn on_close(&mut self, code: CloseCode, reason: &str) {
    	self.inner.lock().unwrap().ui_remote.server_closed(code, reason.to_owned());
    }

    fn on_error(&mut self, err: ws::Error) {
        self.inner.lock().unwrap().ui_remote.server_error(err.into());
    }
}


struct ServerHandlerFactory {
    inner: Arc<Mutex<ServerHandlerInner>>,
}

impl Factory for ServerHandlerFactory {
    type Handler = ServerHandler;

    fn connection_made(&mut self, output: WsSender) -> Self::Handler {
        ServerHandler { inner: self.inner.clone(), output }
    }
}


/// A websocket chat server.
pub struct Server {
    _th: JoinHandle<()>,
    sender: WsSender,
    url: SocketAddr,
}

impl Server {
    pub fn new(url: SocketAddr, ui_remote: UiRemote) -> Server {
        let factory = ServerHandlerFactory {
            inner: Arc::new(Mutex::new(ServerHandlerInner { ui_remote }))
        };

        let ws = WebSocket::new(factory).unwrap();
        let url_clone = url.clone();
        let sender = ws.broadcaster();

        let _th = thread::Builder::new()
                .name("chat-server".to_owned())
                .spawn(move || {
            ws.listen(&url_clone).unwrap();
            info!("Server closing.")
        }).unwrap();

        Server {
            _th,
            sender,
            url,
        }
    }

    pub fn url(&self) -> &SocketAddr {
    	&self.url
    }

    pub fn send<M: Into<Message>>(&self, msg: M) -> Result<(), ws::Error> {
        let ts: Vec<u8> = bincode::serialize(&Pingstamp::now()).unwrap();
        self.sender.send(msg).and(self.sender.send(ts))
    }

    pub fn close_all(&self) -> Result<(), ws::Error>  {
    	self.sender.close(CloseCode::Normal)
    }
}
