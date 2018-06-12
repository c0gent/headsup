//! A websocket chat client.


use std::str;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use bincode;
use url::Url;
use ws::{self, Sender as WsSender, WebSocket, Message, Handler, Handshake, CloseCode, Factory};
use ::{UiRemote, Pingstamp};


struct ClientHandlerInner {
    ui_remote: UiRemote,
}


/// A chat client handler.
struct ClientHandler {
    inner: Arc<Mutex<ClientHandlerInner>>,
    output: WsSender,
}


impl Handler for ClientHandler {
    fn on_shutdown(&mut self) {  }

    fn on_open(&mut self, shake: Handshake) -> Result<(), ws::Error> {
        self.inner.lock().unwrap().ui_remote.client_connected(shake);
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
                        self.inner.lock().unwrap().ui_remote.pong_recvd(ts);
                        Ok(())
                    }
                    Err(_err) => {
                        // self.inner.lock().unwrap().ui_remote.client_error(err.into());
                        Ok(())
                    },
                }
            }
        }
    }

    fn on_close(&mut self, code: CloseCode, reason: &str) {
        self.inner.lock().unwrap().ui_remote.client_closed(code, reason.to_owned());
    }

    fn on_error(&mut self, err: ws::Error) {
        self.inner.lock().unwrap().ui_remote.client_error(err.into());
    }
}


struct ClientHandlerFactory {
    inner: Arc<Mutex<ClientHandlerInner>>,
}


impl Factory for ClientHandlerFactory {
    type Handler = ClientHandler;

    fn connection_made(&mut self, output: WsSender) -> Self::Handler {
        ClientHandler { inner: self.inner.clone(), output }
    }
}


/// A websocket chat client.
pub struct Client {
    _th: JoinHandle<()>,
    sender: WsSender,
    url: Url,
}

impl Client {
    pub fn new(url: Url, ui_remote: UiRemote) -> Client {
        let factory = ClientHandlerFactory {
            inner: Arc::new(Mutex::new(ClientHandlerInner { ui_remote }))
        };

        let mut ws = WebSocket::new(factory).unwrap();

        let sender = ws.broadcaster();

        ws.connect(url.clone()).unwrap();

        let _th = thread::Builder::new()
                .name("chat-client".to_owned())
                .spawn(move || {
            ws.run().unwrap();
            println!("Client closing.")
        }).unwrap();

        Client {
            _th,
            sender,
            url,
        }
    }

    pub fn url(&self) -> &Url {
        &self.url
    }

    pub fn send<M: Into<Message>>(&self, msg: M) -> Result<(), ws::Error> {
        let ts: Vec<u8> = bincode::serialize(&Pingstamp::now()).unwrap();
        self.sender.send(msg).and(self.sender.send(ts))
    }

    pub fn close(&self) -> Result<(), ws::Error>  {
        self.sender.close(CloseCode::Normal)
    }
}
