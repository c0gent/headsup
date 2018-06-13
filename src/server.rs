//! A websocket chat server.

use std::sync::{Arc, Mutex};
use std::collections::BTreeMap;
use std::str;
use std::net::{SocketAddr};
use std::thread::{self, JoinHandle};
use ws::{self, Sender as WsSender, WebSocket, Message, Handler, Handshake, CloseCode, Factory,
	util::Token};
use bincode;
use chrono::Utc;
use ::{UiRemote, Pingstamp, Error};


/// A chat server handler.
struct ServerHandler {
	ui_remote: UiRemote,
    output: WsSender,
    clients: Arc<Mutex<BTreeMap<Token, WsSender>>>,
}

impl Handler for ServerHandler {
    fn on_shutdown(&mut self) {

    }

    fn on_open(&mut self, shake: Handshake) -> Result<(), ws::Error> {
        self.ui_remote.server_connected(shake);
        Ok(())
    }

    fn on_message(&mut self, msg: Message) -> Result<(), ws::Error> {
        match msg {
            Message::Text(s) => {
                // Relay message to other connected clients:
                let cls = self.clients.lock().unwrap();
                for (token, sender) in cls.iter() {
            		if token != &self.output.token() {
            			let send = format!("Client<{}>: {}", usize::from(self.output.token()), s);
            			sender.send(send)?;
            		}
            	}
            	self.ui_remote.message_recvd(s, self.output.token());
                Ok(())
            },
            Message::Binary(b) => {
                match bincode::deserialize::<Pingstamp>(&b) {
                    Ok(Pingstamp::Ping(ts)) => {
                        self.output.send(bincode::serialize(&Pingstamp::Pong(ts)).unwrap())
                    },
                    Ok(Pingstamp::Pong(ts)) => {
                    	let elapsed = Utc::now().signed_duration_since(ts);
                        self.ui_remote.pong_recvd(elapsed);
                        Ok(())
                    }
                    Err(err) => {
                        self.ui_remote.server_error(err.into());
                        Ok(())
                    },
                }
            },
        }
    }

    fn on_close(&mut self, code: CloseCode, reason: &str) {
    	self.ui_remote.server_closed(code, reason.to_owned());
    }

    fn on_error(&mut self, err: ws::Error) {
        self.ui_remote.server_error(err.into());
    }
}


struct ServerHandlerFactory {
    ui_remote: UiRemote,
    // `BTreeSet` because it's faster for a small N.
    clients: Arc<Mutex<BTreeMap<Token, WsSender>>>,
}

impl Factory for ServerHandlerFactory {
    type Handler = ServerHandler;

    fn connection_made(&mut self, output: WsSender) -> Self::Handler {
    	self.clients.lock().unwrap().insert(output.token(), output.clone());
        ServerHandler {
        	ui_remote: self.ui_remote.clone(),
        	output,
        	clients: self.clients.clone(),
        }
    }
}


/// A websocket chat server.
pub struct Server {
    _th: JoinHandle<()>,
    sender: WsSender,
    url: SocketAddr,
}

impl Server {
    pub fn new(url: SocketAddr, ui_remote: UiRemote) -> Result<Server, Error> {
    	let remote_clone = ui_remote.clone();
        let factory = ServerHandlerFactory {
        	ui_remote,
        	clients: Arc::new(Mutex::new(BTreeMap::new())),
    	};
        let ws = WebSocket::new(factory).map_err(Error::from)?;
        let url_clone = url.clone();
        let sender = ws.broadcaster();

        let _th = thread::Builder::new()
                .name("chat-server".to_owned())
                .spawn(move || {
        	let ui_remote = remote_clone;
            if let Err(err) = ws.listen(&url_clone) {
            	ui_remote.server_error(err.into());
            }
            trace!("Server closing.");
        })?;

        Ok(Server {
            _th,
            sender,
            url,
        })
    }

    pub fn url(&self) -> &SocketAddr {
    	&self.url
    }

    pub fn send<M: Into<Message>>(&self, msg: M) -> Result<(), Error> {
        let ts: Vec<u8> = bincode::serialize(&Pingstamp::now())?;
        self.sender.send(msg).and(self.sender.send(ts)).map_err(Error::from)
    }

    pub fn close_all(&self) -> Result<(), Error>  {
    	self.sender.close(CloseCode::Normal).map_err(Error::from)
    }
}
