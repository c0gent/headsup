//! A websocket chat client.


use std::str;
use std::thread::{self, JoinHandle};
use bincode;
use url::Url;
use ws::{self, Sender as WsSender, WebSocket, Message, Handler, Handshake, CloseCode, Factory};
use chrono::Utc;
use ::{UiRemote, Pingstamp, Error};


/// A chat client handler.
struct ClientHandler {
    ui_remote: UiRemote,
    output: WsSender,
}


impl Handler for ClientHandler {
    fn on_shutdown(&mut self) {

    }

    fn on_open(&mut self, shake: Handshake) -> Result<(), ws::Error> {
        self.ui_remote.client_connected(shake);
        Ok(())
    }

    fn on_message(&mut self, msg: Message) -> Result<(), ws::Error> {
        match msg {
            Message::Text(s) => {
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
                        self.ui_remote.client_error(err.into());
                        Ok(())
                    },
                }
            }
        }
    }

    fn on_close(&mut self, code: CloseCode, reason: &str) {
        self.ui_remote.client_closed(code, reason.to_owned());
    }

    fn on_error(&mut self, err: ws::Error) {
        self.ui_remote.client_error(err.into());
    }
}


struct ClientHandlerFactory {
    ui_remote: UiRemote
}


impl Factory for ClientHandlerFactory {
    type Handler = ClientHandler;

    fn connection_made(&mut self, output: WsSender) -> Self::Handler {
        ClientHandler { ui_remote: self.ui_remote.clone(), output }
    }
}


/// A websocket chat client.
pub struct Client {
    _th: JoinHandle<()>,
    sender: WsSender,
    url: Url,
}

impl Client {
    pub fn new(url: Url, ui_remote: UiRemote) -> Result<Client, Error> {
        let remote_clone = ui_remote.clone();
        let factory = ClientHandlerFactory { ui_remote };
        let mut ws = WebSocket::new(factory)?;
        let sender = ws.broadcaster();
        ws.connect(url.clone())?;

        let _th = thread::Builder::new()
                .name("chat-client".to_owned())
                .spawn(move || {
            let ui_remote = remote_clone;
            if let Err(err) = ws.run() {
                ui_remote.client_error(err.into());
            }
            trace!("Client closing.");
        })?;

        Ok(Client {
            _th,
            sender,
            url,
        })
    }

    pub fn url(&self) -> &Url {
        &self.url
    }

    pub fn send<M: Into<Message>>(&self, msg: M) -> Result<(), Error> {
        let ts: Vec<u8> = bincode::serialize(&Pingstamp::now())?;
        self.sender.send(msg).and(self.sender.send(ts)).map_err(Error::from)
    }

    pub fn close(&self) -> Result<(), Error>  {
        self.sender.close(CloseCode::Normal).map_err(Error::from)
    }
}
