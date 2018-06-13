//! A websocket based chat client/server.
//!
//!
//!
//!

extern crate env_logger;
#[macro_use] extern crate failure;
extern crate url;
extern crate clap;
extern crate ws;
extern crate termion;
#[macro_use] extern crate serde_derive;
extern crate bincode;
extern crate chrono;

mod client;
mod server;

use std::mem;
use std::str;
use std::fmt;
use std::time::{Duration};
use std::net::{SocketAddr, ToSocketAddrs};
use std::io::{self, Write,};
use std::thread;
use std::sync::mpsc::{self, Sender as MpscSender, Receiver as MpscReceiver};
use failure::Context;
use termion::{raw::{IntoRawMode, RawTerminal}, event::Key, input::TermRead};
use clap::{App, Arg};
use url::Url;
use ws::{Handshake, CloseCode, util::Token};
use chrono::{DateTime, Utc, serde::ts_nanoseconds};
use client::Client;
use server::Server;


/// Error Kinds.
#[derive(Debug, Fail)]
pub enum ErrorKind {
    #[fail(display = "{}", _0)]
    Fmt(fmt::Error),
    #[fail(display = "{}", _0)]
    Io(io::Error),
    #[fail(display = "Websocket error: {}", _0)]
    Ws(ws::Error),
    #[fail(display = "Codec error: {}", _0)]
    Bincode(Box<bincode::ErrorKind>),
    #[fail(display = "Error parsing url: {}", _0)]
    UrlParse(url::ParseError),
    #[fail(display = "Invalid server address: {}", _0)]
    BadServerAddr(io::Error),
    #[fail(display = "Invalid client address: {}", _0)]
    BadClientAddr(io::Error),
    #[fail(display = "No server address given.")]
    NoServerAddr,
}


/// Errors.
#[derive(Debug)]
pub struct Error {
    inner: Context<ErrorKind>,
}

impl Error {
    pub fn new(kind: ErrorKind) -> Error {
        Error { inner: Context::new(kind) }
    }

    pub fn kind(&self) -> &ErrorKind {
        &self.inner.get_context()
    }

    pub fn bad_server_addr(err: io::Error) -> Error {
        Error::new(ErrorKind::BadServerAddr(err))
    }

    pub fn no_server_addr() -> Error {
        Error::new(ErrorKind::NoServerAddr)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(&self.inner, f)
    }
}

impl From<fmt::Error> for Error {
    fn from(err: fmt::Error) -> Error {
        Error::new(ErrorKind::Fmt(err))
    }
}

impl From<io::Error> for Error {
    fn from(err: io::Error) -> Error {
        Error::new(ErrorKind::Io(err))
    }
}

impl From<ws::Error> for Error {
    fn from(err: ws::Error) -> Error {
        Error::new(ErrorKind::Ws(err))
    }
}

impl From<url::ParseError> for Error {
    fn from(err: url::ParseError) -> Error {
        Error::new(ErrorKind::UrlParse(err))
    }
}

impl From<Box<bincode::ErrorKind>> for Error {
    fn from(err: Box<bincode::ErrorKind>) -> Error {
        Error::new(ErrorKind::Bincode(err))
    }
}



/// Ping timestamp.
#[derive(Debug, Serialize, Deserialize)]
pub enum Pingstamp {
    Ping(#[serde(with = "ts_nanoseconds")] DateTime<Utc>),
    Pong(#[serde(with = "ts_nanoseconds")] DateTime<Utc>),
}

impl Pingstamp {
    pub fn now() -> Pingstamp {
        Pingstamp::Ping(Utc::now())
    }
}


/// The connection state of the ui.
enum ConnectionState {
    ServerListening(Server),
    ServerConnected(Server, usize),
    Client(Client),
    None,
}

#[derive(Debug)]
enum UiCommand {
    ServerOpened(Handshake),
    ServerClosed(CloseCode, String),
    ServerError(Error),
    ClientOpened(Handshake),
    ClientClosed(CloseCode, String),
    ClientError(Error),
    MessageRecvd(String, Token),
    PongRecvd(chrono::Duration),
}


/// A remote control used to send state information to the user interface.
#[derive(Debug, Clone)]
pub struct UiRemote {
    cmd_tx: MpscSender<UiCommand>,
}

impl UiRemote {
    pub fn server_connected(&self, shake: Handshake) {
        self.cmd_tx.send(UiCommand::ServerOpened(shake)).unwrap()
    }

    pub fn server_closed(&self, code: CloseCode, reason: String) {
        self.cmd_tx.send(UiCommand::ServerClosed(code, reason)).unwrap()
    }

    pub fn server_error(&self, err: Error) {
        self.cmd_tx.send(UiCommand::ServerError(err)).unwrap()
    }

    pub fn client_connected(&self, shake: Handshake) {
        self.cmd_tx.send(UiCommand::ClientOpened(shake)).unwrap()
    }

    pub fn client_closed(&self, code: CloseCode, reason: String) {
        self.cmd_tx.send(UiCommand::ClientClosed(code, reason)).unwrap()
    }

    pub fn client_error(&self, err: Error) {
        self.cmd_tx.send(UiCommand::ClientError(err)).unwrap()
    }

    pub fn message_recvd(&self, msg_text: String, token: Token) {
        self.cmd_tx.send(UiCommand::MessageRecvd(msg_text, token)).unwrap()
    }

    pub fn pong_recvd(&self, elapsed: chrono::Duration) {
        self.cmd_tx.send(UiCommand::PongRecvd(elapsed)).unwrap()
    }
}


enum CloseOptions {
    None,
    Decrement,
    Shutdown,
}


/// The console interface.
struct ConsoleUi {
    cmd_tx: MpscSender<UiCommand>,
    cmd_rx: MpscReceiver<UiCommand>,
    conn_state: ConnectionState,
    // If server address is bad it will be set to `None`:
    server_addr: Option<SocketAddr>,
    // Must be stored to keep terminal in raw mode:
    stdout: RawTerminal<io::Stdout>,
    term_size: (u16, u16),
    exit: bool,
}

impl ConsoleUi {
    /// Creates and returns a new console user interface.
    fn new<'s>(server_addr: &'s str, client_addr: Option<Url>) -> Result<ConsoleUi, Error> {
        let (cmd_tx, cmd_rx) = mpsc::channel();

        let server_addr = Some(server_addr.to_socket_addrs()
            .map_err(|err| Error::bad_server_addr(err))?
            .nth(0).ok_or(Error::no_server_addr())?);

        let mut ui = ConsoleUi {
            cmd_tx,
            cmd_rx,
            conn_state: ConnectionState::None,
            server_addr,
            stdout: io::stdout().into_raw_mode()?,
            term_size: termion::terminal_size()?,
            exit: false,
        };

        ui.conn_state = match client_addr {
            Some(cl_addr) => {
                match Client::new(cl_addr.clone(), ui.remote()) {
                    Ok(c) => ConnectionState::Client(c),
                    Err(err) => {
                        ui.output_line(format_args!("Error connecting to client address: {} ({})",
                            cl_addr, err))?;
                        ui.new_server()?
                    },
                }
            },
            None => {
                match Server::new(ui.server_addr.clone().unwrap(), ui.remote()) {
                    Ok(s) => ConnectionState::ServerListening(s),
                    Err(err) => {
                        ui.output_line(format_args!("Unable to connect to serve address: {} ({})",
                            ui.server_addr.as_ref().unwrap(), err))?;
                        ui.server_addr = None;
                        ConnectionState::None
                    },
                }
            },
        };

        ui.output_line(format_args!("Welcome to HeadsUp chat!"))?;
        ui.help()?;
        Ok(ui)
    }

    /// Returns a new `UiRemote` which can send commands and receive events.
    fn remote(&self) -> UiRemote {
        UiRemote { cmd_tx: self.cmd_tx.clone() }
    }

    /// Outputs a formatted line of text.
    fn output_line(&self, args: fmt::Arguments) -> io::Result<()> {
        write!(io::stdout().into_raw_mode()?, "{}{}{}\r\n",
            termion::cursor::Goto(0, self.term_size.1),
            termion::clear::CurrentLine,
            args,
        )
    }

    /// Prints the help message.
    fn help(&self) -> Result <(), Error> {
        self.output_line(format_args!(""))?;
        self.output_line(format_args!("Type '/open {{url}}' or '/connect {{url}}' \
            to connect to a server."))?;
        self.output_line(format_args!("Type '/close' to close the current connection."))?;
        self.output_line(format_args!("Type '/exit' or press ctrl-q to quit."))?;
        self.output_line(format_args!(""))?;
        Ok(())
    }

    /// Outputs the prompt.
    fn output_prompt<'l>(&mut self, line_buf: &'l str) -> Result <(), Error> {
        write!(self.stdout, "{}{}",
            termion::cursor::Goto(0, self.term_size.1),
            termion::clear::CurrentLine,
        )?;
        match self.conn_state {
            ConnectionState::ServerListening(ref s) => write!(self.stdout,
                "[ Listening on ({}) ]> ", s.url()),
            ConnectionState::ServerConnected(_,  cnt) => write!(self.stdout,
                "[ Connected as Server to {} clients ]> ", cnt),
            ConnectionState::Client(ref c) =>  write!(self.stdout,
                "[ Connected as Client to ({}) ]> ", c.url()),
            ConnectionState::None => write!(self.stdout, "[ Disconnected ]> "),
        }?;
        write!(self.stdout, "{}", line_buf)?;
        self.stdout.flush().map_err(Error::from)
    }

    /// If the stored server address is valid, returns a listening connection
    /// state containing a new server.
    fn new_server(&mut self) -> Result <ConnectionState, Error> {
        Ok(match self.server_addr {
            Some(ref sa) => {
                ConnectionState::ServerListening(
                    Server::new(sa.clone(), self.remote())?)
            },
            None => ConnectionState::None,
        })
    }

    /// Sets the connection state as appropriate.
    fn close_connection(&mut self, options: CloseOptions) -> Result <(), Error> {
        self.conn_state = match mem::replace(&mut self.conn_state, ConnectionState::None) {
            ConnectionState::ServerConnected(s, cnt) => {
                if cnt == 0 {
                    ConnectionState::ServerListening(s)
                } else if cnt == 1 {
                    match options {
                        CloseOptions::None => ConnectionState::ServerConnected(s, cnt),
                        CloseOptions::Decrement => ConnectionState::ServerListening(s),
                        CloseOptions::Shutdown => self.new_server()?,

                    }
                } else {
                    match options {
                        CloseOptions::None => ConnectionState::ServerConnected(s, cnt),
                        CloseOptions::Decrement => ConnectionState::ServerConnected(s, cnt - 1),
                        CloseOptions::Shutdown => ConnectionState::ServerListening(s),
                    }
                }
            },
            ConnectionState::ServerListening(s) => {
                match options {
                    CloseOptions::Shutdown => self.new_server()?,
                    _ => ConnectionState::ServerListening(s),
                }
            }
            ConnectionState::Client(_c) => {
                self.new_server()?
            }
            ConnectionState::None => {
                self.new_server()?
            }
        };
        Ok(())
    }

    /// Connects to a server.
    fn connect<'l>(&mut self, l: &'l str) -> Result <(), Error> {
        if let ConnectionState::ServerListening(ref s) = self.conn_state {
            s.shutdown()?;
        }
        match self.conn_state {
            ConnectionState::ServerListening(_) | ConnectionState::None => {
                if let Some(url_str) = l.split(" ").nth(1) {
                    let url = match Url::parse(&format!("ws:{}", url_str)) {
                        Ok(u) => u,
                        Err(_) => {
                            self.output_line(format_args!("Invalid url: '{}'", url_str))?;
                            return Ok(());
                        },
                    };
                    let client = Client::new(url.clone(), self.remote())?;
                    self.conn_state = ConnectionState::Client(client);
                    self.output_line(format_args!("Connecting to: {}...", url))?;
                } else {
                    self.output_line(format_args!("Invalid client URL."))?;
                }
            },
            _ => self.output_line(format_args!("Already connected."))?,
        }
        Ok(())
    }

    /// Closes all current connections.
    fn close_all(&mut self) -> Result <(), Error> {
        match self.conn_state {
            ConnectionState::Client(ref c) => {
                self.output_line(format_args!("Closing connection to server..."))?;
                c.close()?;
            },
            ConnectionState::ServerConnected(ref s, cnt) => {
                self.output_line(format_args!("Closing {} client connections...", cnt))?;
                s.close_all()?;
            },
            _ => self.output_line(format_args!("Not connected."))?,
        }
        Ok(())
    }

    /// Sends a chat message.
    fn send_message<'l>(&mut self, l: &'l str) -> Result <(), Error> {
        let mut close_connection = false;
        match self.conn_state {
            ConnectionState::ServerConnected(ref server, _) => {
                self.output_line(format_args!("{{You (Server)}}: {}", l))?;
                if let Err(err) = server.send(l) {
                    self.output_line(format_args!("Error sending message to client: {}", err))?;
                    close_connection = true;
                }
            },
            ConnectionState::Client(ref client) => {
                self.output_line(format_args!("{{You (Client)}}: {}", l))?;
                if let Err(err) = client.send(l) {
                    self.output_line(format_args!("Error sending message to server: {}", err))?;
                    close_connection = true;
                }
            },
            ConnectionState::None | ConnectionState::ServerListening(..) => {
                self.output_line(format_args!("Cannot send message: '{}'. Not connected.", l))?;
            },
        }
        if close_connection { self.close_connection(CloseOptions::Decrement)?; }
        Ok(())
    }

    /// Handles user input.
    fn handle_input<'l>(&mut self, line: &'l str) -> Result <(), Error> {
        match line {
            "" => {},
            l @ _ => {
                if l.starts_with("/") {
                    if l.starts_with("/open") || l.starts_with("/connect") {
                        self.connect(l)?;
                    } else if l.starts_with("/close") {
                        self.close_all()?;
                    } else if l.starts_with("/exit") {
                        self.exit = true;
                    } else if l.starts_with("/help") {
                        self.help()?;
                    } else {
                        self.output_line(format_args!("Unknown command."))?;
                    }
                } else {
                    self.send_message(l)?;
                }
            }
        }
        self.stdout.flush().map_err(Error::from)
    }

    /// Handles commands sent from server or client.
    fn handle_commands(&mut self) -> Result <(), Error> {
        while let Ok(cmd) = self.cmd_rx.try_recv() {
            match cmd {
                UiCommand::MessageRecvd(m, t) => {
                    match self.conn_state {
                        ConnectionState::ServerConnected(_, _) => {
                            self.output_line(format_args!("{{Client<{}>}}: {}", usize::from(t), m))?;
                        },
                        ConnectionState::Client(_) => {
                            self.output_line(format_args!("{{Server<{}>}}: {}", usize::from(t), m))?;
                        },
                        ConnectionState::None | ConnectionState::ServerListening(..) => {
                            self.output_line(format_args!("{{Unknown}}: {}", m))?;
                        },
                    }
                },
                UiCommand::PongRecvd(elapsed) => {
                    let s = elapsed.num_seconds();
                    let ms = elapsed.num_milliseconds() - (s * 1000);
                    let us = elapsed.num_microseconds().map(|us| us - (s * 1000000)).unwrap_or(ms * 1000);
                    self.output_line(format_args!("    Round-trip: {}.{:06}s", s, us))?;
                },
                UiCommand::ServerOpened(shake) => {
                    if let Some(peer_addr) = shake.peer_addr {
                        match mem::replace(&mut self.conn_state, ConnectionState::None) {
                            ConnectionState::ServerListening(s) => {
                                self.conn_state = ConnectionState::ServerConnected(s, 1);
                            },
                            ConnectionState::ServerConnected(s, cnt) => {
                                self.conn_state = ConnectionState::ServerConnected(s, cnt + 1);
                            },
                            _ => panic!("Invalid connection state."),
                        }
                        self.output_line(format_args!("Server connected to: {}",
                            peer_addr.to_string()))?;
                    } else {
                        self.output_line(format_args!("Server connected.", ))?;
                    }
                },
                UiCommand::ClientOpened(shake) => {
                    if let Some(peer_addr) = shake.peer_addr {
                        self.output_line(format_args!("Client connected to: {}",
                            peer_addr.to_string()))?;
                    } else {
                        panic!("No peer address found.");
                    }
                },
                UiCommand::ClientClosed(_code, reason) => {
                    self.output_line(format_args!("Server connection closed. {}", reason))?;
                    self.close_connection(CloseOptions::None)?;
                },
                UiCommand::ServerClosed(_code, reason) => {
                    self.output_line(format_args!("Client connection closed. {}", reason))?;
                    self.close_connection(CloseOptions::Decrement)?;
                }
                UiCommand::ClientError(err) => {
                    self.output_line(format_args!("The client has encountered an error: {}", err))?;
                    self.close_connection(CloseOptions::Shutdown)?;
                },
                UiCommand::ServerError(err) => {
                    match err.kind() {
                        ErrorKind::Ws(ref err) => match err.kind {
                            ws::ErrorKind::Io(ref err) => match err.kind() {
                                io::ErrorKind::AddrInUse | io::ErrorKind::AddrNotAvailable => {
                                    self.server_addr = None;
                                }
                                _ => {}
                            },
                            _ => {}
                        },
                        _ => {},
                    }
                    self.output_line(format_args!("The server has encountered an error: {}", err))?;
                    self.close_connection(CloseOptions::Shutdown)?;
                }
            }
        }
        Ok(())
    }

    /// Loops, handling events until exit.
    fn run(&mut self) -> Result <(), Error> {
        // Hide cursor and move to lower left of terminal:
        write!(self.stdout.lock().into_raw_mode()?, "{}{}",
            termion::cursor::Hide,
            termion::cursor::Goto(self.term_size.0, 0))?;
        self.output_prompt("")?;

        let mut line_buf = String::new();
        let mut stdin = termion::async_stdin().keys();

        loop {
            if let Err(err) = self.handle_commands() {
                self.output_line(format_args!("Error: {}", err))?;
            }

            match stdin.next() {
                Some(Ok(Key::Ctrl(c))) => {
                    if c == 'q' || c == 'c' {
                        self.exit = true;
                    }
                },
                Some(Ok(Key::Char('\n'))) => {
                    self.handle_input(&line_buf)?;
                    line_buf.clear();
                },
                Some(Ok(Key::Char(c))) => {
                    line_buf.push(c);
                },
                Some(Ok(Key::Backspace)) => {
                    line_buf.pop();
                }
                Some(_) => {},
                None => {},
            }

            if self.exit {
                break;
            } else {
                self.output_prompt(&line_buf)?;
            }

            thread::sleep(Duration::from_millis(10));
        }

        // Reset cursor before exiting:
        write!(self.stdout, "{}{}\n",
                termion::cursor::Goto(0, self.term_size.1),
                termion::cursor::Show)
            .map_err(Error::from)
    }
}


fn main() {
    // Unfortunately the `ws-rs` library does not properly propagate all
    // errors. Logging must be enabled to see error detail for certain things.
    env_logger::init();

    // Parse command line arguments:
    let matches = App::new("Heads-up Chat")
        .version("0.1.0")
        .author("Nick Sanders <nsan1129@gmail.com>")
        .about("Connect or receive chat connections via WebSocket.")
        .arg(Arg::with_name("SERVER")
                .required(false)
                .short("s")
                .long("server")
                .value_name("SERVER")
                .help("Set the address to listen for new connections. Defaults to 'localhost:3030'."))
        .arg(Arg::with_name("CLIENT")
                .required(false)
                .short("c")
                .long("client")
                .value_name("CLIENT")
                .help("Set the remote address to connect to upon startup."))
        .get_matches();

    // Address to listen on upon startup:
    let server_addr = matches.value_of("SERVER").unwrap_or("localhost:3030").to_owned();

    // Address to connect to upon startup:
    let client_addr = match matches.value_of("CLIENT").as_ref()
            .map(|c| Url::parse(&format!("ws:{}", c))) {
        Some(Ok(ca)) => Some(ca),
        Some(Err(err)) => {
            println!("Unable to parse client address: {}", err);
            return;
        },
        None => None,
    };

    // The user interface:
    let mut ui = match ConsoleUi::new(&server_addr, client_addr) {
        Ok(c) => c,
        Err(err) => {
            match err.kind() {
                ErrorKind::BadServerAddr(_) => println!("Invalid server address: '{}'", server_addr),
                ErrorKind::BadClientAddr(_) => println!("Invalid client address: '{}'", server_addr),
                _ => panic!("{}", err),
            }
            return;
        },
    };

    // Run UI:
    ui.run().unwrap();
}