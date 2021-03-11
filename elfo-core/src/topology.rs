use std::{cell::RefCell, sync::Arc};

use crate::{
    addr::Addr,
    address_book::{AddressBook, VacantEntry},
    context::Context,
    demux::{Demux, Filter},
    envelope::Envelope,
    group::Schema,
};

pub struct Topology {
    book: AddressBook,
}

impl Topology {
    pub fn empty() -> Self {
        Self {
            book: AddressBook::new(),
        }
    }

    pub fn local(&self, name: impl Into<String>) -> Local<'_> {
        Local {
            name: name.into(),
            topology: self,
            entry: self.book.vacant_entry(),
            demux: RefCell::new(Demux::default()),
        }
    }

    pub fn remote(&self, _name: impl Into<String>) -> Remote<'_> {
        todo!()
    }
}

impl Default for Topology {
    fn default() -> Self {
        Self::empty()
    }
}

#[must_use]
pub struct Local<'t> {
    name: String,
    topology: &'t Topology,
    entry: VacantEntry<'t>,
    demux: RefCell<Demux>,
}

impl<'t> Local<'t> {
    pub fn route_to(
        &self,
        dest: &impl GetAddrs,
        filter: impl Fn(&Envelope) -> bool + Send + Sync + 'static,
    ) {
        let filter = Arc::new(filter);
        for addr in dest.addrs() {
            self.demux
                .borrow_mut()
                .append(addr, Filter::Dynamic(filter.clone()));
        }
    }

    pub fn route_all_to(&self, dest: &impl GetAddrs) {
        // TODO: more efficient impls.
        self.route_to(dest, |_| true)
    }

    pub async fn mount<M: crate::Message>(self, schema: Schema, msg: Option<M>) {
        let addr = self.entry.addr();
        let book = self.topology.book.clone();
        let root = Context::new(book.clone(), Demux::default());
        let ctx = Context::new(book, self.demux.into_inner()).with_addr(addr);
        let object = (schema.run)(ctx, self.name);
        self.entry.insert(object);

        if let Some(msg) = msg {
            match root.send_to(addr, msg).await {
                Ok(_) => tracing::info!("ok"),
                Err(_) => tracing::error!("fail"),
            }
        }
    }
}

pub struct Remote<'t> {
    addr: Addr,
    _topology: &'t Topology,
}

pub trait GetAddrs {
    fn addrs(&self) -> Vec<Addr>;
}

impl<'t> GetAddrs for Local<'t> {
    fn addrs(&self) -> Vec<Addr> {
        vec![self.entry.addr()]
    }
}

impl<'t> GetAddrs for Remote<'t> {
    fn addrs(&self) -> Vec<Addr> {
        vec![self.addr]
    }
}
