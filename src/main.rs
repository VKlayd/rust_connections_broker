extern crate bytes;
extern crate futures;
extern crate tokio_io;
extern crate tokio_proto;
extern crate tokio_service;
extern crate tokio_core;
extern crate byteorder;
extern crate protobuf;

use tokio_core::reactor::Core;
use tokio_core::net::TcpListener;
use futures::{Future, Stream, Sink};
use std::io;
use bytes::{BytesMut};
use tokio_io::codec::{Encoder, Decoder};
use tokio_io::AsyncRead;
use protobuf::*;
use std::cell::RefCell;
use std::rc::Rc;
use std::collections::{HashMap};
use futures::sync::mpsc::Sender;
//use std::hash::Hash;
use std::hash::Hasher;
use futures::sync::mpsc::{self};

mod header;

pub struct LineCodec;

impl Decoder for LineCodec {
    type Item = header::IpcMessage;
    type Error = io::Error;

    fn decode(&mut self, buf: &mut BytesMut) -> io::Result<Option<header::IpcMessage>> {
        //println!("decode start {}", buf.len());
        if buf.len() < 8 {
            return Ok(None);
        }
        let mut msg = header::IpcMessage::new();
        let read_bytes : u64;

        {
            let mut st = protobuf::stream::CodedInputStream::from_bytes(buf);
            if let Ok(_) = st.merge_message(&mut msg) {

            } else {
                //println!("try again");
                return Ok(None);
            }
            if let Ok(_) = msg.check_initialized() {

            } else {
                //println!("try again");
                return Ok(None);
            }
            read_bytes = st.pos();
        }

        buf.split_to(read_bytes as usize);

        Ok(Some(msg))
    }
}

impl Encoder for LineCodec {
    type Item = header::IpcMessage;
    type Error = io::Error;

    fn encode(&mut self, msg: header::IpcMessage, buf: &mut BytesMut) -> io::Result<()> {
        buf.extend(msg.write_length_delimited_to_bytes()?);
        Ok(())
    }
}

struct Connection {
    send: Sender<header::IpcMessage>,
    tx: u32,
    rx: u32
}

struct Broker {
    connections : HashMap<header::Id, Connection>,
}

impl Eq for header::Id {
}

impl std::hash::Hash for header::Id {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.get_field_type().hash(state);
        self.get_id().hash(state);
    }
}

impl Broker {
    fn new () -> Broker {
        Broker{connections: HashMap::new()}
    }
}

fn serve() -> io::Result<()>
{
    let mut core = Core::new()?;
    let handle = core.handle();

    let address = "0.0.0.0:12345".parse().unwrap();
    let listener = TcpListener::bind(&address, &handle)?;

    let connections = listener.incoming();

    let broker_root = Rc::new(RefCell::new(Broker::new()));
    let broker = broker_root.clone();

    let handle_inner = core.handle();
    let server = connections.for_each(move |(socket, _peer_addr)| {
        println!("new connection?");
        let framed = socket.framed(LineCodec);
        let broker_h = broker.clone();
        let handshake = framed.into_future()
            .map_err(move |(err, _)| {
                println!("pre handshake error = {:?}", err);
                err
            }) // for accept errors, get error and discard the stream
            .and_then(move |(packet,framed)| { // only accepted connections from here
                let mut broker_in = broker_h.borrow_mut();
                if let Some(packet) = packet {
                    let (tx, rx) = mpsc::channel::<header::IpcMessage>(10);
                    broker_in.connections.insert(packet.get_header().get_sender()[0].clone(), Connection{send: tx, tx: 0, rx:0});
                    Ok((framed, rx))
                } else {
                    println!("shit");
                    Err(io::Error::new(io::ErrorKind::Other, "Invalid Handshake Packet"))
                }
            });
        let broker_c = broker.clone();
        let broker_s = broker.clone();
        let handle_inner = handle_inner.clone();
        let mqtt = handshake.map(|w| {
            Some(w)
        })
            .map(move |handshake| {
                if let Some((framed, rx)) = handshake {
                    let (sender, receiver) = framed.split();
                    let rx_fut = receiver.or_else(|e| {
                        println!("Receiver error = {:?}", e);
                        Err::<_, ()>(())
                    }).for_each(move |msg : header::IpcMessage| {
                        let recv_id = msg.get_header().get_receiver()[0].clone();
                        let broker1 = broker_c.clone();
                        {
                            let mut broker_in = broker_c.borrow_mut();
                            let s = broker_in.connections.get_mut(&msg.get_header().get_sender()[0].clone());
                            if let Some(sx) = s {
                                sx.rx += 1;
                                if sx.rx % 100000 == 0 {
                                    println!("{:?} rx = {}", msg.get_header().get_sender(), sx.rx);
                                }
                            } else {}
                        }
                        {
                            let mut broker_in = broker1.borrow_mut();
                            let s = broker_in.connections.get_mut(&recv_id);
                            if let Some(sx) = s {
                                //sx.tx += 1;
                                sx.send.clone().send(msg).poll();
                            } else {}
                        }
                        Ok(())
                    });


                    let tx_fut = rx
                        .map_err(move |e| {std::io::Error::new(std::io::ErrorKind::Other, "FF")})
                        .and_then(move |e| {
                            let mut broker_in = broker_s.borrow_mut();
                            let s = broker_in.connections.get_mut(&e.get_header().get_sender()[0].clone());
                            if let Some(sx) = s {
                                sx.tx += 1;
                                if sx.tx % 100000 == 0 {
                                    println!("{:?} tx = {}", e.get_header().get_sender(), sx.tx);
                                }
                            } else {
                            }
                            Ok(e)
                        } )
                        .forward(sender)
                        .map_err(move |e| {
                            println!("transmission error = {:?}", e);
                            Ok::<_, ()>(())
                        })
                        .then(move |_| {
                        Ok::<_, ()>(())
                    });
                    //let connection = rx_fut.select(tx_fut).then(move|_| {Ok::<_, ()>(())});
                    handle_inner.spawn(tx_fut);
                    handle_inner.spawn(rx_fut);
                }
                Ok::<_, ()>(())
            }).then(|_| Ok(()));


        handle.spawn(mqtt);
        Ok(())
    });


    core.run(server)
}

fn main() {
    if let Err(e) = serve() {
        println!("Server failed with {}", e);
    }
}
