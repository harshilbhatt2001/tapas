//! Google Calendar: busy intervals on the primary calendar, the scheduler's view of free time.

use chrono::{DateTime, Utc};
use google_calendar3::CalendarHub;
use google_calendar3::api::{FreeBusyRequest, FreeBusyRequestItem};
use hyper_rustls::HttpsConnectorBuilder;
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("loading the system's TLS roots")]
    Roots(#[from] std::io::Error),
    #[error("Google Calendar request failed")]
    Api(#[from] Box<google_calendar3::Error>),
    #[error("Google Calendar could not read the primary calendar: {0}")]
    Calendar(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Busy {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

/// Busy intervals on the primary calendar between `from` and `to`.
pub async fn busy(
    access_token: String,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> Result<Vec<Busy>, Error> {
    let connector = HttpsConnectorBuilder::new()
        .with_native_roots()?
        .https_only()
        .enable_http1()
        .enable_http2()
        .build();
    let hub = CalendarHub::new(
        Client::builder(TokioExecutor::new()).build(connector),
        access_token,
    );
    let request = FreeBusyRequest {
        time_min: Some(from),
        time_max: Some(to),
        items: Some(vec![FreeBusyRequestItem {
            id: Some("primary".into()),
        }]),
        ..Default::default()
    };
    let (_, response) = hub
        .freebusy()
        .query(request)
        .doit()
        .await
        .map_err(Box::new)?;
    let calendar = response
        .calendars
        .and_then(|mut calendars| calendars.remove("primary"))
        .unwrap_or_default();
    if let Some(error) = calendar.errors.into_iter().flatten().next() {
        return Err(Error::Calendar(error.reason.unwrap_or_default()));
    }
    Ok(calendar
        .busy
        .into_iter()
        .flatten()
        .filter_map(|period| {
            Some(Busy {
                start: period.start?,
                end: period.end?,
            })
        })
        .collect())
}
