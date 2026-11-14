//! The Steam thread the window asks: it holds the session, answers commands,
//! notices Steam going away and coming back, and watches downloads.
//!
//! Steamworks wants its calls and its callbacks on one thread, so everything
//! is here. The thread is a [`Worker`] stepped by a loop ([`spawn`]); the
//! worker takes the time it is given rather than reading the clock, so that
//! tests can step it through minutes of Steam's life at once.

use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::{Duration, Instant};

use super::{Entry, Error, ItemState, PAGE_SIZE, Page, Persona, Progress, Query, Steam, Vote, Workshop};
use crate::lang::t;

/// How long after a failed start Steam is looked for again.
const RETRY: Duration = Duration::from_secs(5);
/// How often Steam is checked to still be there.
const RUNNING_CHECK: Duration = Duration::from_secs(1);
/// How often a watched download is asked how far it is.
const POLL: Duration = Duration::from_millis(500);
/// How often the list of subscriptions is read again: an item may be
/// subscribed to, unsubscribed from or updated by Steam itself, or from its
/// client or the website.
const REFRESH: Duration = Duration::from_secs(5);
/// Steam takes a moment to mark a new subscription: an item without the
/// mark is watched for this long before it counts as not subscribed.
const MARK_GRACE: Duration = Duration::from_secs(20);
/// How long Steam is given to fetch a stranger's name.
const PERSONA_WAIT: Duration = Duration::from_secs(10);

pub enum Command {
    Query {
        generation: u64,
        query: Query,
    },
    /// The workshop's details of installed items.
    Details(Vec<u64>),
    /// Who an author is.
    Persona(u64),
    /// The user's vote on an item.
    Vote(u64),
    SetVote {
        id: u64,
        up: bool,
    },
    Favorite {
        id: u64,
        on: bool,
    },
    Subscribe(u64),
    Unsubscribe(u64),
    Quit,
}

#[derive(Debug, PartialEq)]
pub enum Reply {
    /// Steam is there, or why not (said again only when the reason changes).
    Session(Result<(), String>),
    Page {
        generation: u64,
        result: Result<Page, String>,
    },
    Details(Vec<Entry>),
    Persona {
        id: u64,
        persona: Persona,
    },
    /// The user's vote, as Steam has it.
    Vote {
        id: u64,
        vote: Vote,
    },
    /// A vote given, or why not.
    Voted {
        id: u64,
        result: Result<Vote, String>,
    },
    Favorite {
        id: u64,
        on: bool,
        result: Result<(), String>,
    },
    Subscribed {
        id: u64,
        result: Result<(), String>,
    },
    Unsubscribed {
        id: u64,
        result: Result<(), String>,
    },
    Subscriptions(Vec<(u64, ItemState)>),
    Progress {
        id: u64,
        progress: Progress,
    },
}

type Say<'a> = &'a mut dyn FnMut(Reply);

pub(super) struct Worker<W, O> {
    open: O,
    steam: Option<W>,
    retry_at: Instant,
    /// The last reason given for having no session.
    said: Option<String>,
    /// What waited for Steam, in the order it was asked for.
    pending: Vec<Command>,
    /// Downloads followed, with since when.
    watched: Vec<(u64, Instant)>,
    /// Authors Steam is fetching, with when they were asked for.
    asked: Vec<(u64, Instant)>,
    /// The subscriptions as last told.
    known: Vec<(u64, ItemState)>,
    polled: Instant,
    refreshed: Instant,
    looked: Instant,
}

impl<W: Workshop, O: FnMut() -> Result<W, Error>> Worker<W, O> {
    pub(super) fn new(open: O, now: Instant) -> Self {
        Self {
            open,
            steam: None,
            retry_at: now,
            said: None,
            pending: Vec::new(),
            watched: Vec::new(),
            asked: Vec::new(),
            known: Vec::new(),
            polled: now,
            refreshed: now,
            looked: now,
        }
    }

    #[cfg(test)]
    pub(super) fn connected(&self) -> bool {
        self.steam.is_some()
    }

    /// Start the session when there is none and it is time to try.
    pub(super) fn connect(&mut self, now: Instant, say: Say) {
        if self.steam.is_some() || now < self.retry_at {
            return;
        }
        match (self.open)() {
            Ok(steam) => {
                self.said = None;
                say(Reply::Session(Ok(())));
                let list = steam.subscribed();
                self.steam = Some(steam);
                self.looked = now;
                self.adopt(&list, now);
                self.known = list.clone();
                self.refreshed = now;
                say(Reply::Subscriptions(list));
            }
            Err(e) => self.no_session(e, now, say),
        }
    }

    fn no_session(&mut self, e: Error, now: Instant, say: Say) {
        self.retry_at = now + RETRY;
        let why = e.to_string();
        if self.said.as_deref() != Some(why.as_str()) {
            say(Reply::Session(Err(why.clone())));
            self.said = Some(why);
        }
    }

    /// Steam went away: the session is let go of, to be started again.
    fn lose(&mut self, now: Instant, say: Say) {
        self.steam = None;
        self.asked.clear();
        self.no_session(Error::Gone, now, say);
    }

    /// What waited for Steam, now that it is there.
    pub(super) fn next_pending(&mut self) -> Option<Command> {
        if self.steam.is_some() && !self.pending.is_empty() { Some(self.pending.remove(0)) } else { None }
    }

    /// Answer one command, or none (a tick), and look after what is watched.
    /// False once told to quit.
    pub(super) fn handle(&mut self, now: Instant, command: Option<Command>, say: Say) -> bool {
        match command {
            Some(Command::Quit) => return false,
            Some(command) if self.steam.is_none() => self.wait_for_steam(command, say),
            Some(command) => self.run(command, now, say),
            None => {}
        }
        self.tick(now, say);
        true
    }

    /// No session: a list is answered with why (the window asks again when
    /// Steam is back), anything else waits for Steam.
    fn wait_for_steam(&mut self, command: Command, say: Say) {
        match command {
            Command::Query { generation, .. } => {
                let why = self.said.clone().unwrap_or_else(|| Error::Gone.to_string());
                say(Reply::Page { generation, result: Err(why) });
            }
            Command::Persona(id)
                if self.pending.iter().any(|p| matches!(p, Command::Persona(a) if *a == id)) => {}
            command => self.pending.push(command),
        }
    }

    fn run(&mut self, command: Command, now: Instant, say: Say) {
        let Some(steam) = self.steam.as_mut() else { return };
        let words = |r: Result<(), Error>| r.map_err(|e| e.to_string());
        let mut gone = false;
        match command {
            Command::Query { generation, query } => {
                let result = steam.query(&query);
                gone = is_gone(&result);
                say(Reply::Page { generation, result: result.map_err(|e| e.to_string()) });
            }
            Command::Details(ids) => {
                // A page of the workshop is fifty at most.
                for chunk in ids.chunks(PAGE_SIZE as usize) {
                    let result = steam.details(chunk);
                    gone = is_gone(&result);
                    let Ok(page) = result else { break };
                    say(Reply::Details(page.entries));
                }
            }
            Command::Persona(id) => {
                // Steam fetches a stranger first: asked again for a while.
                if let Some(persona) = steam.persona(id) {
                    say(Reply::Persona { id, persona });
                } else if !self.asked.iter().any(|(a, _)| *a == id) {
                    self.asked.push((id, now));
                }
            }
            Command::Vote(id) => {
                let result = steam.vote(id);
                gone = is_gone(&result);
                if let Ok(vote) = result {
                    say(Reply::Vote { id, vote });
                }
            }
            Command::SetVote { id, up } => {
                let result = steam.set_vote(id, up);
                gone = is_gone(&result);
                let vote = if up { Vote::Up } else { Vote::Down };
                say(Reply::Voted { id, result: words(result).map(|()| vote) });
            }
            Command::Favorite { id, on } => {
                let result = steam.set_favorite(id, on);
                gone = is_gone(&result);
                say(Reply::Favorite { id, on, result: words(result) });
            }
            Command::Subscribe(id) => {
                let result = steam.subscribe(id);
                gone = is_gone(&result);
                if result.is_ok() {
                    // Steam fetches a new subscription by itself, in its own
                    // time; asked to now, it starts at once.
                    steam.download(id);
                    if !self.watched.iter().any(|(w, _)| *w == id) {
                        self.watched.push((id, now));
                    }
                }
                say(Reply::Subscribed { id, result: words(result) });
            }
            Command::Unsubscribe(id) => {
                let result = steam.unsubscribe(id);
                gone = is_gone(&result);
                let done = result.is_ok();
                say(Reply::Unsubscribed { id, result: words(result) });
                if done {
                    self.watched.retain(|(w, _)| *w != id);
                    let list = steam.subscribed();
                    self.known = list.clone();
                    say(Reply::Subscriptions(list));
                }
            }
            Command::Quit => {}
        }
        if gone {
            self.lose(now, say);
        }
    }

    /// Between commands: is Steam still there, the callbacks, the authors
    /// asked for, the downloads, and the subscriptions changed elsewhere.
    fn tick(&mut self, now: Instant, say: Say) {
        if self.steam.is_none() {
            return;
        }
        if now.duration_since(self.looked) >= RUNNING_CHECK {
            self.looked = now;
            if !self.steam.as_ref().is_some_and(|s| s.running()) {
                self.lose(now, say);
                return;
            }
        }
        let Some(steam) = self.steam.as_mut() else { return };
        steam.pump();
        self.asked.retain(|&(id, since)| match steam.persona(id) {
            Some(persona) => {
                say(Reply::Persona { id, persona });
                false
            }
            None => now.duration_since(since) < PERSONA_WAIT,
        });
        let mut landed = false;
        if !self.watched.is_empty() && now.duration_since(self.polled) >= POLL {
            self.polled = now;
            let mut lost = Vec::new();
            self.watched.retain(|&(id, since)| {
                let progress = steam.progress(id);
                say(Reply::Progress { id, progress });
                let finished = progress.state.finished();
                landed |= finished;
                let gone = !progress.state.subscribed() && now.duration_since(since) > MARK_GRACE;
                if gone && !finished {
                    lost.push(id);
                }
                !finished && !gone
            });
            // The subscription did not hold: nothing is coming.
            for id in lost {
                say(Reply::Subscribed {
                    id,
                    result: Err(t("Steam did not keep the subscription", "Steam не сохранил подписку").into()),
                });
            }
        }
        if landed || now.duration_since(self.refreshed) >= REFRESH {
            self.refreshed = now;
            let list = steam.subscribed();
            self.adopt(&list, now);
            if landed || list != self.known {
                self.known = list.clone();
                say(Reply::Subscriptions(list));
            }
        }
    }

    /// Downloads Steam started on its own (an update, a subscription made
    /// elsewhere) are followed as ours are.
    fn adopt(&mut self, list: &[(u64, ItemState)], now: Instant) {
        for &(id, state) in list {
            if state.subscribed() && state.downloading() && !self.watched.iter().any(|(w, _)| *w == id) {
                self.watched.push((id, now));
            }
        }
    }
}

fn is_gone<T>(result: &Result<T, Error>) -> bool {
    matches!(result, Err(Error::Gone))
}

/// The Steam thread: starts the session (and again every few seconds while
/// there is none), answers commands and looks after the session in between.
pub fn spawn(reply: impl Fn(Reply) + Send + 'static) -> Sender<Command> {
    let (tx, rx): (Sender<Command>, Receiver<Command>) = channel();
    std::thread::Builder::new()
        .name("steam".into())
        .spawn(move || {
            let mut say = |r: Reply| reply(r);
            let mut worker = Worker::new(Steam::open, Instant::now());
            loop {
                worker.connect(Instant::now(), &mut say);
                let command = match worker.next_pending() {
                    Some(c) => Some(c),
                    None => match rx.recv_timeout(Duration::from_millis(100)) {
                        Ok(c) => Some(c),
                        Err(RecvTimeoutError::Timeout) => None,
                        Err(RecvTimeoutError::Disconnected) => return,
                    },
                };
                if !worker.handle(Instant::now(), command, &mut say) {
                    return;
                }
            }
        })
        .expect("steam thread");
    tx
}

#[cfg(test)]
mod tests {
    use super::super::fake::{Fake, World, entries};
    use super::super::sdk::result;
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    type Open = Box<dyn FnMut() -> Result<Fake, Error>>;

    /// A worker on a fake Steam, with its own clock.
    struct Bed {
        world: Rc<RefCell<World>>,
        worker: Worker<Fake, Open>,
        now: Instant,
        /// What was said as the thread started.
        first: Vec<Reply>,
    }

    impl Bed {
        fn new(world: World) -> Self {
            let world = Rc::new(RefCell::new(world));
            let opened = world.clone();
            let open: Open = Box::new(move || Fake::open(&opened));
            let now = Instant::now();
            let mut bed = Self { world, worker: Worker::new(open, now), now, first: Vec::new() };
            bed.first = bed.step(None);
            bed
        }

        /// Steam running, with these subscriptions.
        fn running(items: &[(u64, ItemState)]) -> Self {
            Self::new(World { running: true, items: items.iter().copied().collect(), ..World::default() })
        }

        /// One turn of the thread's loop, then the replies of it.
        fn step(&mut self, command: Option<Command>) -> Vec<Reply> {
            let mut out = Vec::new();
            let mut say = |r: Reply| out.push(r);
            self.worker.connect(self.now, &mut say);
            // What waited for Steam goes first, as in the thread.
            while let Some(waiting) = self.worker.next_pending() {
                assert!(self.worker.handle(self.now, Some(waiting), &mut say));
            }
            assert!(self.worker.handle(self.now, command, &mut say));
            out
        }

        fn send(&mut self, command: Command) -> Vec<Reply> {
            self.step(Some(command))
        }

        /// Time passes, the thread ticking as it does.
        fn wait(&mut self, time: Duration) -> Vec<Reply> {
            let mut out = Vec::new();
            let end = self.now + time;
            while self.now < end {
                self.now += Duration::from_millis(100).min(end - self.now);
                out.extend(self.step(None));
            }
            out
        }

        fn world(&self) -> std::cell::RefMut<'_, World> {
            self.world.borrow_mut()
        }
    }

    fn sessions(replies: &[Reply]) -> Vec<&Result<(), String>> {
        replies.iter().filter_map(|r| if let Reply::Session(s) = r { Some(s) } else { None }).collect()
    }

    fn subscriptions(replies: &[Reply]) -> Vec<&Vec<(u64, ItemState)>> {
        replies.iter().filter_map(|r| if let Reply::Subscriptions(l) = r { Some(l) } else { None }).collect()
    }

    fn progress_of(replies: &[Reply], id: u64) -> Vec<Progress> {
        replies
            .iter()
            .filter_map(|r| match r {
                Reply::Progress { id: of, progress } if *of == id => Some(*progress),
                _ => None,
            })
            .collect()
    }

    fn query(page: u32) -> Query {
        Query {
            sort: super::super::Sort::Vote,
            text: String::new(),
            page,
            groups: Vec::new(),
            required: Vec::new(),
            excluded: Vec::new(),
            trend_days: 1,
            author: 0,
        }
    }

    const INSTALLED: ItemState = ItemState(ItemState::SUBSCRIBED.0 | ItemState::INSTALLED.0);
    const DOWNLOADING: ItemState = ItemState(ItemState::SUBSCRIBED.0 | ItemState::DOWNLOADING.0);

    #[test]
    fn a_session_starts_with_the_subscriptions() {
        let mut bed = Bed::running(&[(1, INSTALLED)]);
        assert_eq!(bed.first, [Reply::Session(Ok(())), Reply::Subscriptions(vec![(1, INSTALLED)])]);
        assert!(bed.wait(Duration::from_secs(1)).is_empty());
    }

    #[test]
    fn a_subscription_steam_takes_is_downloaded_and_lands() {
        let mut bed = Bed::running(&[]);
        bed.world().subscribe_gives.insert(7, DOWNLOADING);
        let replies = bed.send(Command::Subscribe(7));
        assert_eq!(replies, [Reply::Subscribed { id: 7, result: Ok(()) }]);
        // The download is asked for right after Steam took the subscription.
        assert_eq!(bed.world().calls, ["subscribe 7", "download 7"]);

        bed.world().downloads.insert(7, (40, 100));
        let replies = bed.wait(Duration::from_millis(600));
        assert_eq!(progress_of(&replies, 7), [Progress { state: DOWNLOADING, done: 40, total: 100 }]);

        bed.world().items.insert(7, INSTALLED);
        bed.world().downloads.insert(7, (100, 100));
        let replies = bed.wait(Duration::from_millis(600));
        assert_eq!(progress_of(&replies, 7).last().map(|p| p.state), Some(INSTALLED));
        // Landed: the list again, with it, so the window reads the disk.
        assert_eq!(subscriptions(&replies), [&vec![(7, INSTALLED)]]);
        // And it is no longer followed.
        assert!(progress_of(&bed.wait(Duration::from_secs(2)), 7).is_empty());
    }

    #[test]
    fn a_subscription_steam_refuses_is_said_and_nothing_is_downloaded() {
        let mut bed = Bed::running(&[]);
        bed.world().refuse.insert(8, Error::Refused(result::ACCESS_DENIED));
        let replies = bed.send(Command::Subscribe(8));
        assert_eq!(
            replies,
            [Reply::Subscribed { id: 8, result: Err("Steam says this account may not do that".into()) }]
        );
        assert_eq!(bed.world().calls, ["subscribe 8"]);
        assert!(progress_of(&bed.wait(Duration::from_secs(2)), 8).is_empty());
    }

    #[test]
    fn a_subscription_steam_does_not_keep_is_given_up_on() {
        let mut bed = Bed::running(&[]);
        // Taken, but Steam never marks the item subscribed.
        bed.world().subscribe_gives.insert(9, ItemState::default());
        bed.send(Command::Subscribe(9));
        let early = bed.wait(Duration::from_secs(10));
        assert!(!early.iter().any(|r| matches!(r, Reply::Subscribed { .. })));
        let late = bed.wait(Duration::from_secs(12));
        assert!(late.iter().any(|r| matches!(r, Reply::Subscribed { id: 9, result: Err(_) })));
        assert!(progress_of(&bed.wait(Duration::from_secs(2)), 9).is_empty());
    }

    #[test]
    fn unsubscribing_answers_and_lists_again() {
        let mut bed = Bed::running(&[(1, INSTALLED), (2, INSTALLED)]);
        let replies = bed.send(Command::Unsubscribe(2));
        assert_eq!(
            replies,
            [Reply::Unsubscribed { id: 2, result: Ok(()) }, Reply::Subscriptions(vec![(1, INSTALLED)])]
        );
        bed.world().refuse.insert(1, Error::Refused(result::BUSY));
        let replies = bed.send(Command::Unsubscribe(1));
        assert!(matches!(replies.as_slice(), [Reply::Unsubscribed { id: 1, result: Err(_) }]));
        assert!(bed.world().items.contains_key(&1));
    }

    #[test]
    fn a_query_gets_the_page_it_asks_for() {
        let mut bed = Bed::running(&[]);
        bed.world().catalogue = entries(120);
        let replies = bed.send(Command::Query { generation: 3, query: query(3) });
        let [Reply::Page { generation: 3, result: Ok(page) }] = replies.as_slice() else {
            panic!("{replies:?}");
        };
        assert_eq!(page.total, 120);
        assert_eq!(page.entries.len(), 20);
        assert_eq!(page.entries[0].id, 101);
        assert_eq!(bed.world().queries.last().map(|q| q.page), Some(3));
    }

    #[test]
    fn a_failed_query_says_why_and_the_session_stays() {
        let mut bed = Bed::running(&[]);
        bed.world().query_fails = Some(Error::Refused(result::NO_CONNECTION));
        let replies = bed.send(Command::Query { generation: 1, query: query(1) });
        assert_eq!(
            replies,
            [Reply::Page {
                generation: 1,
                result: Err("Steam is offline: no connection to its servers".into())
            }]
        );
        assert!(bed.worker.connected());
    }

    #[test]
    fn details_are_asked_fifty_at_a_time() {
        let mut bed = Bed::running(&[]);
        bed.world().catalogue = entries(120);
        let replies = bed.send(Command::Details((1..=120).collect()));
        let sizes: Vec<usize> = replies
            .iter()
            .filter_map(|r| if let Reply::Details(d) = r { Some(d.len()) } else { None })
            .collect();
        assert_eq!(sizes, [50, 50, 20]);
    }

    #[test]
    fn votes_are_read_given_and_refused() {
        let mut bed = Bed::running(&[]);
        bed.world().votes.insert(5, Vote::Up);
        assert_eq!(bed.send(Command::Vote(5)), [Reply::Vote { id: 5, vote: Vote::Up }]);
        assert_eq!(
            bed.send(Command::SetVote { id: 5, up: false }),
            [Reply::Voted { id: 5, result: Ok(Vote::Down) }]
        );
        assert_eq!(bed.world().votes.get(&5), Some(&Vote::Down));
        bed.world().refuse.insert(5, Error::Refused(result::LIMIT_EXCEEDED));
        let replies = bed.send(Command::SetVote { id: 5, up: true });
        assert!(matches!(replies.as_slice(), [Reply::Voted { id: 5, result: Err(_) }]));
        assert_eq!(bed.world().votes.get(&5), Some(&Vote::Down));
    }

    #[test]
    fn favourites_are_given_taken_back_and_refused() {
        let mut bed = Bed::running(&[]);
        assert_eq!(
            bed.send(Command::Favorite { id: 6, on: true }),
            [Reply::Favorite { id: 6, on: true, result: Ok(()) }]
        );
        assert!(bed.world().favorites.contains(&6));
        bed.send(Command::Favorite { id: 6, on: false });
        assert!(!bed.world().favorites.contains(&6));
        bed.world().refuse.insert(6, Error::NoAnswer);
        let replies = bed.send(Command::Favorite { id: 6, on: true });
        assert!(matches!(replies.as_slice(), [Reply::Favorite { id: 6, on: true, result: Err(_) }]));
    }

    #[test]
    fn without_steam_it_says_so_once_and_starts_when_steam_does() {
        let mut bed = Bed::new(World::default());
        assert!(!bed.worker.connected());
        assert_eq!(bed.world().opens, 1);

        // A list is answered at once, with why; a subscription waits.
        let replies = bed.send(Command::Query { generation: 1, query: query(1) });
        assert_eq!(replies, [Reply::Page { generation: 1, result: Err("Steam is not running".into()) }]);
        assert!(bed.send(Command::Subscribe(4)).is_empty());

        // Looked for again every few seconds, said only once.
        let quiet = bed.wait(Duration::from_secs(12));
        assert!(sessions(&quiet).is_empty(), "{quiet:?}");
        assert_eq!(bed.world().opens, 3);

        bed.world().running = true;
        bed.world().subscribe_gives.insert(4, DOWNLOADING);
        let back = bed.wait(Duration::from_secs(5));
        assert_eq!(sessions(&back), [&Ok(())]);
        assert_eq!(subscriptions(&back).first(), Some(&&vec![]));
        assert!(back.contains(&Reply::Subscribed { id: 4, result: Ok(()) }));
    }

    #[test]
    fn the_reason_is_said_again_when_it_changes() {
        let mut bed =
            Bed::new(World { refuse_open: Some("libsteam_api.so was not found".into()), ..World::default() });
        bed.world().refuse_open = None;
        let replies = bed.wait(Duration::from_secs(5));
        assert_eq!(sessions(&replies), [&Err("Steam is not running".to_string())]);
    }

    #[test]
    fn steam_going_away_ends_the_session_and_it_comes_back() {
        let mut bed = Bed::running(&[(1, INSTALLED)]);
        bed.world().running = false;
        let replies = bed.wait(Duration::from_secs(1));
        assert_eq!(sessions(&replies), [&Err("Steam was closed".to_string())]);
        assert!(!bed.worker.connected());
        // The session was shut down, not left to call into a dead client.
        assert_eq!(bed.world().shutdowns, 1);

        // Asked meanwhile: it waits.
        assert!(bed.send(Command::Favorite { id: 1, on: true }).is_empty());
        bed.world().running = true;
        let back = bed.wait(Duration::from_secs(5));
        assert_eq!(sessions(&back), [&Ok(())]);
        assert!(back.contains(&Reply::Favorite { id: 1, on: true, result: Ok(()) }));
        assert_eq!(bed.world().opens, 2);
    }

    #[test]
    fn steam_going_away_during_a_call_ends_the_session() {
        let mut bed = Bed::running(&[]);
        bed.world().query_fails = Some(Error::Gone);
        let replies = bed.send(Command::Query { generation: 2, query: query(1) });
        assert!(matches!(replies.first(), Some(Reply::Page { generation: 2, result: Err(_) })));
        assert!(matches!(replies.last(), Some(Reply::Session(Err(_)))));
        assert!(!bed.worker.connected());
    }

    #[test]
    fn an_item_changed_elsewhere_is_told() {
        let mut bed = Bed::running(&[(1, INSTALLED), (2, INSTALLED)]);
        // Unsubscribed from on the website.
        bed.world().items.remove(&2);
        let replies = bed.wait(Duration::from_secs(5));
        assert_eq!(subscriptions(&replies), [&vec![(1, INSTALLED)]]);

        // Its author updated it: Steam downloads it again by itself.
        let updating = INSTALLED.with(ItemState::DOWNLOADING);
        bed.world().items.insert(1, updating);
        bed.world().downloads.insert(1, (5, 10));
        let replies = bed.wait(Duration::from_secs(6));
        assert!(progress_of(&replies, 1).contains(&Progress { state: updating, done: 5, total: 10 }));

        bed.world().items.insert(1, INSTALLED);
        let replies = bed.wait(Duration::from_secs(1));
        assert_eq!(subscriptions(&replies).last(), Some(&&vec![(1, INSTALLED)]));
        // Nothing changed since: nothing more is said.
        let quiet = bed.wait(Duration::from_secs(11));
        assert!(subscriptions(&quiet).is_empty(), "{quiet:?}");
    }

    #[test]
    fn an_author_is_told_once_steam_has_fetched_them() {
        let mut bed = Bed::running(&[]);
        assert!(bed.send(Command::Persona(11)).is_empty());
        bed.world().personas.insert(11, "Mika".into());
        let replies = bed.wait(Duration::from_millis(200));
        assert_eq!(
            replies,
            [Reply::Persona { id: 11, persona: Persona { name: "Mika".into(), avatar: None } }]
        );
        // One Steam never finds is given up on.
        bed.send(Command::Persona(12));
        bed.wait(Duration::from_secs(11));
        bed.world().personas.insert(12, "late".into());
        assert!(bed.wait(Duration::from_secs(1)).is_empty());
    }

    #[test]
    fn quit_stops_the_thread() {
        let mut bed = Bed::running(&[]);
        let mut say = |_: Reply| {};
        assert!(!bed.worker.handle(bed.now, Some(Command::Quit), &mut say));
    }
}
