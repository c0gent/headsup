//! A websocket based chat client/server.
//!

#[macro_use] extern crate log;
extern crate env_logger;
extern crate url;
extern crate clap;
extern crate ws;
extern crate termion;
#[macro_use] extern crate serde_derive;
extern crate bincode;
extern crate chrono;
#[macro_use] extern crate failure;


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
use termion::{raw::{IntoRawMode, RawTerminal}, event::Key, input::TermRead};
use clap::{App, Arg};
use url::Url;
use ws::{Handshake, CloseCode};
use chrono::{DateTime, Utc, serde::ts_nanoseconds};
use client::Client;
use server::Server;


/// Errors.
///
/// TODO: Beef up error handling and explicitly handle more cases.
#[derive(Debug, Fail)]
pub enum Error {
    #[fail(display = "{}", _0)]
    Ws(ws::Error),
    #[fail(display = "{}", _0)]
    Bincode(Box<bincode::ErrorKind>),
}

impl From<ws::Error> for Error {
    fn from(err: ws::Error) -> Error {
        Error::Ws(err)
    }
}

impl From<Box<bincode::ErrorKind>> for Error {
    fn from(err: Box<bincode::ErrorKind>) -> Error {
        Error::Bincode(err)
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
    ServerConnected(Server),
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
    MessageRecvd(String),
    PongRecvd(DateTime<Utc>),
}


/// A remote control used to send state information to the user interface.
#[derive(Debug)]
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

    pub fn message_recvd(&self, msg_text: String) {
        self.cmd_tx.send(UiCommand::MessageRecvd(msg_text)).unwrap()
    }

    pub fn pong_recvd(&self, ts: DateTime<Utc>) {
        self.cmd_tx.send(UiCommand::PongRecvd(ts)).unwrap()
    }
}


/// The console interface.
struct ConsoleUi {
    cmd_tx: MpscSender<UiCommand>,
    cmd_rx: MpscReceiver<UiCommand>,
    conn_state: ConnectionState,
    server_addr: SocketAddr,
    // Must be stored to keep terminal in raw mode:
    stdout: RawTerminal<io::Stdout>,
    term_size: (u16, u16),
    exit: bool,
}

impl ConsoleUi {
    /// Creates and returns a new console user interface.
    fn new<'s>(server_addr: &'s str, client_addr: Option<Url>) -> ConsoleUi {
        let (cmd_tx, cmd_rx) = mpsc::channel();

        let mut ui = ConsoleUi {
            cmd_tx,
            cmd_rx,
            conn_state: ConnectionState::None,
            server_addr: server_addr.to_socket_addrs().unwrap().nth(0).expect("Bad socket address"),
            stdout: io::stdout().into_raw_mode().unwrap(),
            term_size: termion::terminal_size().unwrap(),
            exit: false,
        };

        ui.conn_state = match client_addr {
            Some(cl_addr) => ConnectionState::Client(
                Client::new(cl_addr, ui.client_remote())),
            None => ConnectionState::ServerListening(
                Server::new(ui.server_addr.clone(), ui.server_remote())),
        };

        ui.help();
        ui
    }

    /// Returns a new `UiRemote` which can send commands and receive events.
    fn server_remote(&mut self) -> UiRemote {
        UiRemote { cmd_tx: self.cmd_tx.clone() }
    }

    /// Returns a new `UiRemote` which can send commands and receive events.
    fn client_remote(&mut self) -> UiRemote {
        UiRemote { cmd_tx: self.cmd_tx.clone() }
    }

    /// Outputs a formatted line of text.
    fn output_line(&self, args: fmt::Arguments) -> io::Result<()> {
        write!(io::stdout().into_raw_mode().unwrap(), "{}{}{}\r\n",
            termion::cursor::Goto(0, self.term_size.1),
            termion::clear::CurrentLine,
            args,
        )
    }

    /// Prints the help message.
    fn help(&self) {
        self.output_line(format_args!("Type '/connect' {{url}} to connect to a server.")).unwrap();
        self.output_line(format_args!("Type '/close' to close the current connection.")).unwrap();
        self.output_line(format_args!("Type '/exit' or press ctrl-q to quit.")).unwrap();
    }

    /// Outputs the prompt.
    fn output_prompt<'l>(&mut self, line_buf: &'l str) {
        write!(self.stdout, "{}{}",
            termion::cursor::Goto(0, self.term_size.1),
            termion::clear::CurrentLine,
        ).unwrap();
        match self.conn_state {
            ConnectionState::ServerListening(ref s) => write!(self.stdout, "[ Listening on ({}) ]> ", s.url()),
            ConnectionState::ServerConnected(ref s) => write!(self.stdout, "[ Connected as Server to ({}) ]> ", s.url()),
            ConnectionState::Client(ref c) =>  write!(self.stdout, "[ Connected as Client to ({}) ]> ", c.url()),
            ConnectionState::None => write!(self.stdout, "[ Disconnected ]> "),
        }.unwrap();
        write!(self.stdout, "{}", line_buf).unwrap();
        self.stdout.flush().unwrap();
    }

    /// Sets the connection state to `ServerListening`.
    fn listen(&mut self) {
        self.conn_state = match mem::replace(&mut self.conn_state, ConnectionState::None) {
            ConnectionState::ServerConnected(s) => ConnectionState::ServerListening(s),
            ConnectionState::ServerListening(s) => ConnectionState::ServerListening(s),
            ConnectionState::Client(_) | ConnectionState::None => {
                ConnectionState::ServerListening(
                    Server::new(self.server_addr.clone(), self.server_remote()))
            }
        };
    }

    /// Handles user input.
    fn handle_input<'l>(&mut self, line: &'l str) {
        match line {
            "" => {},
            l @ _ => {
                if l.starts_with("/") {
                    if l.starts_with("/connect") {
                        if let ConnectionState::ServerListening(_) = self.conn_state {
                            if let Some(url) = l.split(" ").nth(1) {
                                let url = Url::parse(&format!("ws:{}", url)).unwrap();
                                let client = Client::new(url.clone(), self.client_remote());
                                self.conn_state = ConnectionState::Client(client);
                                self.output_line(format_args!("Connecting to: {}...", url)).unwrap();
                            } else {
                                panic!("Invalid client URL.");
                            }
                        } else {
                            self.output_line(format_args!("Already connected.")).unwrap();
                        }
                    } else if l.starts_with("/close") {
                        match self.conn_state {
                            ConnectionState::Client(_) => {
                                self.output_line(format_args!("Server connection closed.")).unwrap();
                            },
                            ConnectionState::ServerConnected(_) => {
                                self.output_line(format_args!("Client connection closed.")).unwrap();
                            },
                            _ => self.output_line(format_args!("Not connected.")).unwrap(),
                        }
                    } else if l.starts_with("/exit") {
                        self.exit = true;
                    } else if l.starts_with("/help") {
                        self.help();
                    } else {
                        self.output_line(format_args!("Unknown command.")).unwrap();
                    }
                } else {
                    match self.conn_state {
                        ConnectionState::ServerConnected(ref server) => {
                            self.output_line(format_args!("{{You (Server)}}: {}", l)).unwrap();
                            server.send(l).unwrap();
                        },
                        ConnectionState::Client(ref client) => {
                            self.output_line(format_args!("{{You (Client)}}: {}", l)).unwrap();
                            client.send(l).unwrap();
                        },
                        ConnectionState::None | ConnectionState::ServerListening(..) => {
                            self.output_line(format_args!("Cannot send message: '{}'. Not connected.", l)).unwrap();
                        },
                    }
                }
            }
        }

        self.stdout.flush().unwrap();
    }

    /// Handles commands sent from server or client.
    fn handle_commands(&mut self) {
        while let Ok(cmd) = self.cmd_rx.try_recv() {
            match cmd {
                UiCommand::MessageRecvd(m) => {
                    match self.conn_state {
                        ConnectionState::ServerConnected(ref server) => {
                            self.output_line(format_args!("{{Client ({})}}: {}", server.url(), m)).unwrap();
                        },
                        ConnectionState::Client(ref client) => {
                            self.output_line(format_args!("{{Server ({})}}: {}", client.url(), m)).unwrap();
                        },
                        ConnectionState::None | ConnectionState::ServerListening(..) => {
                            self.output_line(format_args!("{{Unknown}}: {}", m)).unwrap();
                        },
                    }
                },
                UiCommand::PongRecvd(ts) => {
                    let elapsed = Utc::now().signed_duration_since(ts);
                    let s = elapsed.num_seconds();
                    let ms = elapsed.num_milliseconds() - (s * 1000);
                    let us = elapsed.num_microseconds().map(|us| us - (s * 1000000)).unwrap_or(ms * 1000);
                    self.output_line(format_args!("    Round-trip: {}.{:06}s", s, us)).unwrap();
                },
                UiCommand::ServerOpened(shake) => {
                    if let Some(peer_addr) = shake.peer_addr {
                        match mem::replace(&mut self.conn_state, ConnectionState::None) {
                            ConnectionState::ServerListening(s) => {
                                self.conn_state = ConnectionState::ServerConnected(s);
                            },
                            ConnectionState::ServerConnected(s) => {
                                self.output_line(format_args!("Multiple connections detected. \
                                    Multiple client connections are not yet fully supported!", )).unwrap();
                                self.conn_state = ConnectionState::ServerConnected(s);
                            },
                            _ => panic!("Invalid connection state."),
                        }
                        self.output_line(format_args!("Server connected to: {}",
                            peer_addr.to_string())).unwrap();
                    } else {
                        self.output_line(format_args!("Server connected.", )).unwrap();
                    }
                },
                UiCommand::ClientOpened(shake) => {
                    if let Some(peer_addr) = shake.peer_addr {
                        self.output_line(format_args!("Client connected to: {}",
                            peer_addr.to_string())).unwrap();
                    } else {
                        panic!("No peer address found.");
                    }
                },
                UiCommand::ClientClosed(_code, reason) => {
                    self.output_line(format_args!("Server connection closed. {}", reason)).unwrap();
                    self.listen();

                },
                UiCommand::ServerClosed(_code, reason) => {
                    self.output_line(format_args!("Client connection closed. {}", reason)).unwrap();
                    self.listen();
                }
                UiCommand::ClientError(err) => {
                    self.output_line(format_args!("The client has encountered an error: {}", err)).unwrap();
                    self.listen();

                },
                UiCommand::ServerError(err) => {
                    self.output_line(format_args!("The server has encountered an error: {}", err)).unwrap();
                    self.listen();
                }
            }
        }
    }

    /// Loops, handling events, until exit.
    fn run(&mut self) {
        // Hide cursor and move to lower left of terminal:
        write!(self.stdout.lock().into_raw_mode().unwrap(), "{}{}",
            termion::cursor::Hide,
            termion::cursor::Goto(self.term_size.0, 0)).unwrap();
        self.output_prompt("");

        let mut line_buf = String::new();
        let mut stdin = termion::async_stdin().keys();

        loop {
            self.handle_commands();

            match stdin.next() {
                Some(Ok(Key::Ctrl(c))) => {
                    if c == 'q' || c == 'c' {
                        self.exit = true;
                    }
                },
                Some(Ok(Key::Char('\n'))) => {
                    self.handle_input(&line_buf);
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
                self.output_prompt(&line_buf);
            }

            thread::sleep(Duration::from_millis(10));
        }

        // Reset cursor before exiting:
        write!(self.stdout, "{}{}\n",
            termion::cursor::Goto(0, self.term_size.1),
            termion::cursor::Show).unwrap();
    }
}



fn main() {
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
    let client_addr = matches.value_of("CLIENT").as_ref()
        .map(|c| Url::parse(&format!("ws:{}", c)).unwrap());

    // The user interface:
    let mut ui = ConsoleUi::new(&server_addr, client_addr);

    // Run UI:
    ui.run();
}