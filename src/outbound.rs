use std::time::Duration;

use futures_util::{future::select, pin_mut};
use worker::{AbortController, Delay, Fetch, Request, Response, Result};

pub async fn fetch_with_timeout(request: Request, timeout: Duration) -> Result<Option<Response>> {
    let controller = AbortController::default();
    let signal = controller.signal();
    let fetch_request = Fetch::Request(request);
    let fetch = fetch_request.send_with_signal(&signal);
    let delay = Delay::from(timeout);
    pin_mut!(fetch, delay);

    match select(fetch, delay).await {
        futures_util::future::Either::Left((response, _)) => response.map(Some),
        futures_util::future::Either::Right(((), _)) => {
            controller.abort();
            Ok(None)
        }
    }
}
