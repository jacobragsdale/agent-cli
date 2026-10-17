//! What the crate's tests share: a Redis that answers from a script on a
//! port of its own, so handler tests run the real door over a real socket,
//! and a run of the domain against it.

use std::collections::HashMap;
use std::io::{BufReader, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use agent_cli_core::Setup;
use agent_cli_core::testing::{FakeTransport, Outcome, run};

use crate::DOMAIN;
use crate::session::{Reply, read_reply};

/// A scripted server: each command, its words joined by spaces, gets its
/// reply; anything unscripted gets an error naming it. `sent` is every
/// command it was asked, in order.
pub(crate) struct Fake {
    pub port: u16,
    pub sent: Arc<Mutex<Vec<String>>>,
}

pub(crate) fn serve(script: Vec<(&str, Reply)>) -> Fake {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let mut answers: HashMap<String, Reply> = script
        .into_iter()
        .map(|(command, reply)| (command.to_owned(), reply))
        .collect();
    answers
        .entry("CLIENT SETNAME agent-cli".to_owned())
        .or_insert(Reply::Status("OK".into()));
    let answers = Arc::new(answers);
    let sent = Arc::new(Mutex::new(Vec::new()));
    let log = sent.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let (answers, log) = (answers.clone(), log.clone());
            std::thread::spawn(move || {
                let mut writer = stream.try_clone().unwrap();
                let mut reader = BufReader::new(stream);
                while let Ok(command) = read_reply(&mut reader) {
                    let words: Vec<String> = command
                        .items()
                        .iter()
                        .map(|word| word.text().unwrap_or_default())
                        .collect();
                    let line = words.join(" ");
                    log.lock().unwrap().push(line.clone());
                    let reply = answers
                        .get(&line)
                        .cloned()
                        .unwrap_or_else(|| Reply::Error(format!("ERR unscripted: {line}")));
                    let mut wire = Vec::new();
                    encode_reply(&reply, &mut wire);
                    if writer.write_all(&wire).is_err() {
                        break;
                    }
                }
            });
        }
    });
    Fake { port, sent }
}

impl Fake {
    pub fn sent(&self) -> Vec<String> {
        self.sent.lock().unwrap().clone()
    }
}

fn encode_reply(reply: &Reply, wire: &mut Vec<u8>) {
    match reply {
        Reply::Status(text) => wire.extend_from_slice(format!("+{text}\r\n").as_bytes()),
        Reply::Error(text) => wire.extend_from_slice(format!("-{text}\r\n").as_bytes()),
        Reply::Int(value) => wire.extend_from_slice(format!(":{value}\r\n").as_bytes()),
        Reply::Nil => wire.extend_from_slice(b"$-1\r\n"),
        Reply::Bulk(bytes) => {
            wire.extend_from_slice(format!("${}\r\n", bytes.len()).as_bytes());
            wire.extend_from_slice(bytes);
            wire.extend_from_slice(b"\r\n");
        }
        Reply::Array(items) => {
            wire.extend_from_slice(format!("*{}\r\n", items.len()).as_bytes());
            for item in items {
                encode_reply(item, wire);
            }
        }
    }
}

pub(crate) fn bulk(text: impl AsRef<[u8]>) -> Reply {
    Reply::Bulk(text.as_ref().to_vec())
}

pub(crate) fn array(items: impl IntoIterator<Item = Reply>) -> Reply {
    Reply::Array(items.into_iter().collect())
}

/// One instance per `(name, port)`, plain and without a password.
pub(crate) fn config(instances: &[(&str, u16)]) -> String {
    instances
        .iter()
        .map(|(name, port)| {
            format!("[[redis.instance]]\nname = \"{name}\"\nhost = \"127.0.0.1\"\nport = {port}\n")
        })
        .collect()
}

pub(crate) fn redis(argv: &[&str], config: &str) -> Outcome {
    run(
        &[DOMAIN],
        argv,
        Setup::fake(FakeTransport::default()).with_config(config),
    )
}
