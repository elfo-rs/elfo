use std::{collections::VecDeque, sync::Arc, time::Instant as StdInstant};

use parking_lot::Mutex;
use slotmap::{DefaultKey, SlotMap};
use tokio::{task, time::Duration};

use crate::{
    context::Context,
    group::ActorGroup,
    mailbox::{Envelope, Message, MessageKind, SendError},
    object::Addr,
};

const MAX_WAIT_TIME: Duration = Duration::from_millis(100);

pub struct Mock {
    state: Arc<Mutex<State>>,
    context: Context,
}

type CheckFn = Box<dyn Fn(&Envelope) + Send>;
type AnswerFn = Box<dyn Fn(Envelope) -> Envelope + Send>;

struct State {
    check_fns: SlotMap<DefaultKey, CheckFn>,
    answer_fn: AnswerFn,
    input: Option<VecDeque<Envelope>>,
}

impl Mock {
    pub fn new(ctx: &Context) -> (Addr, Self) {
        let state = Arc::new(Mutex::new(State {
            check_fns: SlotMap::new(),
            answer_fn: Box::new(|_| {
                panic!("A mock cannot answer, you should use `Mock::answers()`")
            }),
            input: None,
        }));

        let state1 = state.clone();
        let addr = ActorGroup::new("mock")
            .exec(move |ctx| {
                let state = state1.clone();
                async move {
                    while let Some(mut envelope) = ctx.recv().await {
                        let mut state = state.lock();

                        for (_, check_fn) in state.check_fns.iter() {
                            check_fn(&envelope);
                        }

                        if let Some(tx) = envelope.downgrade_to_reqular() {
                            let _ = tx.send((state.answer_fn)(envelope));
                        } else if let Some(input) = &mut state.input {
                            input.push_back(envelope);
                        }
                    }
                }
            })
            .spawn(ctx);

        let context = ctx.child(addr);
        (addr, Mock { state, context })
    }

    pub fn add_check(&self, f: impl Fn(&Envelope) + Send + 'static) -> CheckHandle {
        let mut state = self.state.lock();
        let check_id = state.check_fns.insert(Box::new(f));

        CheckHandle {
            check_id,
            state: self.state.clone(),
        }
    }

    pub fn answers<R: Message>(&self, f: impl Fn(Envelope) -> R + Send + 'static) {
        let mut state = self.state.lock();
        state.answer_fn = Box::new(move |envelope| {
            let answer = f(envelope);
            let payload = smallbox::smallbox!(answer);
            Envelope::new(Addr::default(), payload, MessageKind::regular()) // TODO: default?
        });
    }

    pub async fn send_to<M: Message>(&self, addr: Addr, message: M) -> Result<(), SendError<M>> {
        self.context.send_to(addr, message).await
    }

    pub async fn ask<M: Message>(
        &self,
        addr: Addr,
        message: M,
        // TODO: how to avoid `Option`?
    ) -> Result<Envelope, SendError<Option<M>>> {
        self.context.ask(addr, message).await
    }

    pub fn record_input(&self) -> InputIter {
        let mut state = self.state.lock();
        state.input = Some(VecDeque::new());
        InputIter {
            state: self.state.clone(),
        }
    }
}

pub struct CheckHandle {
    check_id: DefaultKey,
    state: Arc<Mutex<State>>,
}

impl CheckHandle {
    pub fn cancel(&self) {
        let mut state = self.state.lock();
        state.check_fns.remove(self.check_id);
    }
}

pub struct InputIter {
    state: Arc<Mutex<State>>,
}

impl InputIter {
    pub async fn recv(&mut self) -> Envelope {
        // We are forced to use `std::time::Instant` instead of `tokio::time::Instant`
        // because we don't want to use mocked time by tokio.
        let start = StdInstant::now();

        while {
            if let Some(envelope) = self.try_recv() {
                return envelope;
            }

            task::yield_now().await; // TODO: what about `std::thread::yield_now`?
            start.elapsed() < MAX_WAIT_TIME
        } {}

        panic!("Too long");
    }

    pub fn try_recv(&mut self) -> Option<Envelope> {
        let mut state = self.state.lock();
        let input = state.input.as_mut().expect("Invalid InputIter");
        input.pop_front()
    }
}

impl Drop for InputIter {
    fn drop(&mut self) {
        let mut state = self.state.lock();
        state.input = None;
    }
}
