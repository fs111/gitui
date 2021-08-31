use crate::{
	error::{Error, Result},
	sync::{
		cred::BasicAuthCredential,
		remotes::{fetch, push::ProgressNotification},
	},
	AsyncGitNotification, RemoteProgress, CWD,
};
use crossbeam_channel::{unbounded, Sender};
use std::{
	sync::{Arc, Mutex},
	thread,
};

///
#[derive(Default, Clone, Debug)]
pub struct FetchRequest {
	///
	pub remote: String,
	///
	pub branch: String,
	///
	pub basic_credential: Option<BasicAuthCredential>,
}

#[derive(Default, Clone, Debug)]
struct FetchState {
	request: FetchRequest,
}

///
pub struct AsyncFetch {
	state: Arc<Mutex<Option<FetchState>>>,
	last_result: Arc<Mutex<Option<(usize, String)>>>,
	progress: Arc<Mutex<Option<ProgressNotification>>>,
	sender: Sender<AsyncGitNotification>,
}

impl AsyncFetch {
	///
	pub fn new(sender: &Sender<AsyncGitNotification>) -> Self {
		Self {
			state: Arc::new(Mutex::new(None)),
			last_result: Arc::new(Mutex::new(None)),
			progress: Arc::new(Mutex::new(None)),
			sender: sender.clone(),
		}
	}

	///
	pub fn is_pending(&self) -> Result<bool> {
		let state = self.state.lock()?;
		Ok(state.is_some())
	}

	///
	pub fn last_result(&self) -> Result<Option<(usize, String)>> {
		let res = self.last_result.lock()?;
		Ok(res.clone())
	}

	///
	pub fn progress(&self) -> Result<Option<RemoteProgress>> {
		let res = self.progress.lock()?;
		Ok(res.as_ref().map(|progress| progress.clone().into()))
	}

	///
	pub fn request(&mut self, params: FetchRequest) -> Result<()> {
		log::trace!("request: {}/{}", params.remote, params.branch);

		if self.is_pending()? {
			log::trace!("request ignored, still pending");
			return Ok(());
		}

		self.set_request(&params)?;
		RemoteProgress::set_progress(&self.progress, None)?;

		let arc_state = Arc::clone(&self.state);
		let arc_res = Arc::clone(&self.last_result);
		let arc_progress = Arc::clone(&self.progress);
		let sender = self.sender.clone();

		thread::spawn(move || {
			let res = Self::threaded_fetch(
				params,
				&sender,
				arc_progress,
				&arc_res,
				&arc_state,
			);

			if let Err(e) = res {
				log::error!("async fetch: {}", e);
				Self::clear_request(&arc_state)
					.expect("clear request error");
			}

			sender
				.send(AsyncGitNotification::Fetch)
				.expect("notification error");
		});

		Ok(())
	}

	fn set_request(&self, params: &FetchRequest) -> Result<()> {
		let mut state = self.state.lock()?;

		if state.is_some() {
			return Err(Error::Generic("pending request".into()));
		}

		*state = Some(FetchState {
			request: params.clone(),
		});

		Ok(())
	}

	fn clear_request(
		state: &Arc<Mutex<Option<FetchState>>>,
	) -> Result<()> {
		let mut state = state.lock()?;

		*state = None;

		Ok(())
	}

	fn set_result(
		arc_result: &Arc<Mutex<Option<(usize, String)>>>,
		res: Result<usize>,
	) -> Result<()> {
		let mut last_res = arc_result.lock()?;

		*last_res = match res {
			Ok(bytes) => Some((bytes, String::new())),
			Err(e) => {
				log::error!("fetch error: {}", e);
				Some((0, e.to_string()))
			}
		};

		Ok(())
	}

	fn threaded_fetch(
		params: FetchRequest,
		sender: &Sender<AsyncGitNotification>,
		arc_progress: Arc<Mutex<Option<ProgressNotification>>>,
		arc_res: &Arc<Mutex<Option<(usize, String)>>>,
		arc_state: &Arc<Mutex<Option<FetchState>>>,
	) -> Result<()> {
		let (progress_sender, receiver) = unbounded();

		let handle = RemoteProgress::spawn_receiver_thread(
			AsyncGitNotification::Fetch,
			sender.clone(),
			receiver,
			arc_progress,
		);

		let res = fetch(
			CWD,
			&params.branch,
			params.basic_credential,
			Some(progress_sender.clone()),
		);

		progress_sender.send(ProgressNotification::Done)?;

		handle.join()?;

		Self::set_result(arc_res, res)?;
		Self::clear_request(arc_state)?;

		Ok(())
	}
}
