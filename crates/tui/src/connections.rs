//! The daemon connections the TUI's requests share, so they don't each
//! connect and ask `Status` first: usually the first load's alone, for the
//! whole session.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc::Sender;

use chatgpt_core::{ErrorKind, Paths};
use chatgpt_launcher::{ClientError, DaemonClient, Inspection};
use chatgpt_protocol::{DaemonStatus, Event as DaemonEvent, Request, ResponseData};

use crate::run::Event;

/// A request takes an idle connection, or opens one when none is idle, and
/// gives it back once answered.
pub struct Connections {
    paths: Paths,
    idle: RefCell<Vec<DaemonClient>>,
    events: Sender<Event>,
}

/// More idle connections than this are closed rather than kept.
const MOST_IDLE: usize = 2;

impl Connections {
    pub fn new(paths: Paths, first: DaemonClient, events: Sender<Event>) -> Rc<Self> {
        Rc::new(Self {
            paths,
            idle: RefCell::new(vec![first]),
            events,
        })
    }

    fn give_back(&self, client: DaemonClient) {
        let mut idle = self.idle.borrow_mut();
        if idle.len() < MOST_IDLE {
            idle.push(client);
        }
    }

    /// A new connection, starting the daemon (or a newer one) if needed.
    async fn open(&self) -> Result<DaemonClient, ClientError> {
        let opened = chatgpt_launcher::connect(&self.paths).await;
        let _ = self.events.send(Event::Repaint);
        opened.map(|(client, _)| client)
    }

    /// `request`'s answer. An idle connection may have closed with a
    /// daemon that stopped or restarted since: a read sent on one is sent
    /// again on a new connection if the old one fails, and a write (which
    /// mustn't be sent twice) goes out only after `Status` shows the
    /// connection still answers.
    pub async fn ask(
        &self,
        request: Request,
        mut on_event: impl FnMut(DaemonEvent),
    ) -> Result<ResponseData, ClientError> {
        let read = matches!(
            request,
            Request::Status | Request::List { .. } | Request::Transcript { .. }
        );
        let idle = self.idle.borrow_mut().pop();
        let (mut client, reused) = match idle {
            Some(client) => (client, true),
            None => (self.open().await?, false),
        };
        if reused
            && !read
            && client
                .request_with_events(Request::Status, |_| {})
                .await
                .is_err()
        {
            client = self.open().await?;
        }
        let again = (reused && read).then(|| request.clone());
        match client.request_with_events(request, &mut on_event).await {
            Err(error) if error.kind == ErrorKind::DaemonUnavailable => {
                let Some(request) = again else {
                    return Err(error);
                };
                let mut client = self.open().await?;
                let answer = client.request_with_events(request, on_event).await;
                if answer.is_ok() {
                    self.give_back(client);
                }
                answer
            }
            answer => {
                self.give_back(client);
                answer
            }
        }
    }

    /// The daemon's status, without starting one: a TUI left open doesn't
    /// bring back a daemon that was stopped.
    pub async fn status(&self) -> Option<DaemonStatus> {
        let idle = self.idle.borrow_mut().pop();
        if let Some(mut client) = idle
            && let Ok(ResponseData::Status(status)) =
                client.request_with_events(Request::Status, |_| {}).await
        {
            self.give_back(client);
            return Some(*status);
        }
        match chatgpt_launcher::inspect(&self.paths).await {
            Inspection::Ready(status) => Some(*status),
            Inspection::Unhealthy { .. } | Inspection::Stopped => None,
        }
    }
}
