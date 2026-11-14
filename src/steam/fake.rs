//! A Steam for tests: a [`World`] the test sets up and changes as it goes
//! (Steam starting and quitting, items changing state, refusals), and the
//! [`Fake`] session on it that the worker talks to.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;

use super::{Entry, Error, ItemState, PAGE_SIZE, Page, Persona, Progress, Query, Vote, Workshop};

#[derive(Default)]
pub struct World {
    /// The Steam client is running: a session starts, and one going stays.
    pub running: bool,
    /// Starting a session fails with this, Steam running or not.
    pub refuse_open: Option<String>,
    /// Subscriptions and their states.
    pub items: BTreeMap<u64, ItemState>,
    /// The state an item has right after Steam took a subscription to it
    /// (subscribed and downloading when not said).
    pub subscribe_gives: HashMap<u64, ItemState>,
    /// Bytes done and in all, of downloads.
    pub downloads: HashMap<u64, (u64, u64)>,
    /// What Steam answers about these items, whatever is asked.
    pub refuse: HashMap<u64, Error>,
    /// Every query fails with this.
    pub query_fails: Option<Error>,
    /// The whole workshop, paged by queries.
    pub catalogue: Vec<Entry>,
    pub votes: HashMap<u64, Vote>,
    pub favorites: HashSet<u64>,
    /// Names Steam has fetched.
    pub personas: HashMap<u64, String>,
    /// What was asked to be done, in order ("subscribe 7", "download 7").
    pub calls: Vec<String>,
    pub queries: Vec<Query>,
    pub opens: u32,
    pub shutdowns: u32,
}

pub struct Fake {
    world: Rc<RefCell<World>>,
}

impl Fake {
    pub fn open(world: &Rc<RefCell<World>>) -> Result<Self, Error> {
        let mut w = world.borrow_mut();
        w.opens += 1;
        if let Some(why) = &w.refuse_open {
            return Err(Error::Unavailable(why.clone()));
        }
        if !w.running {
            return Err(Error::Unavailable("Steam is not running".into()));
        }
        Ok(Self { world: world.clone() })
    }

    /// What Steam says to an action on `id`: its refusal, or that it is gone.
    fn answer(&self, id: u64) -> Result<(), Error> {
        let w = self.world.borrow();
        if !w.running {
            return Err(Error::Gone);
        }
        w.refuse.get(&id).map_or(Ok(()), |e| Err(e.clone()))
    }
}

/// `n` workshop items with the ids 1 to n.
pub fn entries(n: u64) -> Vec<Entry> {
    (1..=n).map(|id| Entry { id, title: format!("item {id}"), ..Entry::default() }).collect()
}

impl Workshop for Fake {
    fn pump(&mut self) {}

    fn running(&self) -> bool {
        self.world.borrow().running
    }

    fn query(&mut self, query: &Query) -> Result<Page, Error> {
        let mut w = self.world.borrow_mut();
        w.queries.push(query.clone());
        if let Some(e) = w.query_fails.clone() {
            return Err(e);
        }
        let start = (query.page.max(1) as usize - 1) * PAGE_SIZE as usize;
        let entries = w.catalogue.iter().skip(start).take(PAGE_SIZE as usize).cloned().collect();
        Ok(Page { entries, total: w.catalogue.len() as u32 })
    }

    fn details(&mut self, ids: &[u64]) -> Result<Page, Error> {
        assert!(ids.len() <= PAGE_SIZE as usize, "Steam takes fifty ids at most");
        let w = self.world.borrow();
        let entries: Vec<Entry> = w.catalogue.iter().filter(|e| ids.contains(&e.id)).cloned().collect();
        Ok(Page { total: entries.len() as u32, entries })
    }

    fn subscribe(&mut self, id: u64) -> Result<(), Error> {
        self.world.borrow_mut().calls.push(format!("subscribe {id}"));
        self.answer(id)?;
        let mut w = self.world.borrow_mut();
        let state = w
            .subscribe_gives
            .get(&id)
            .copied()
            .unwrap_or(ItemState(ItemState::SUBSCRIBED.0 | ItemState::DOWNLOADING.0));
        w.items.insert(id, state);
        Ok(())
    }

    fn unsubscribe(&mut self, id: u64) -> Result<(), Error> {
        self.world.borrow_mut().calls.push(format!("unsubscribe {id}"));
        self.answer(id)?;
        self.world.borrow_mut().items.remove(&id);
        Ok(())
    }

    fn download(&mut self, id: u64) -> bool {
        self.world.borrow_mut().calls.push(format!("download {id}"));
        true
    }

    fn subscribed(&self) -> Vec<(u64, ItemState)> {
        let w = self.world.borrow();
        w.items.iter().filter(|(_, s)| s.subscribed()).map(|(&id, &s)| (id, s)).collect()
    }

    fn progress(&self, id: u64) -> Progress {
        let w = self.world.borrow();
        let (done, total) = w.downloads.get(&id).copied().unwrap_or((0, 0));
        Progress { state: w.items.get(&id).copied().unwrap_or_default(), done, total }
    }

    fn vote(&mut self, id: u64) -> Result<Vote, Error> {
        self.answer(id)?;
        Ok(self.world.borrow().votes.get(&id).copied().unwrap_or(Vote::None))
    }

    fn set_vote(&mut self, id: u64, up: bool) -> Result<(), Error> {
        self.answer(id)?;
        self.world.borrow_mut().votes.insert(id, if up { Vote::Up } else { Vote::Down });
        Ok(())
    }

    fn set_favorite(&mut self, id: u64, on: bool) -> Result<(), Error> {
        self.answer(id)?;
        let mut w = self.world.borrow_mut();
        if on {
            w.favorites.insert(id);
        } else {
            w.favorites.remove(&id);
        }
        Ok(())
    }

    fn persona(&mut self, id: u64) -> Option<Persona> {
        let name = self.world.borrow().personas.get(&id).cloned()?;
        Some(Persona { name, avatar: None })
    }
}

impl Drop for Fake {
    fn drop(&mut self) {
        self.world.borrow_mut().shutdowns += 1;
    }
}
