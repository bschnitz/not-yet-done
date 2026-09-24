//! Online meetings: which service an event is held on, and the link that joins
//! it.
//!
//! A backend that knows (Graph's `onlineMeeting`, a CalDAV `CONFERENCE`
//! property) says so directly; for everything else [`detect`] reads the links
//! and phrases in the event's text. Both end up as the same [`OnlineMeeting`].

/// The service an online meeting is held on. `Other` is a meeting the source
/// flagged as online without saying where, or a link no rule knows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MeetingProvider {
    Teams,
    Zoom,
    Meet,
    Webex,
    Jitsi,
    Other,
}

impl MeetingProvider {
    /// Stable lowercase token (column values, the event store).
    pub fn as_str(self) -> &'static str {
        match self {
            MeetingProvider::Teams => "teams",
            MeetingProvider::Zoom => "zoom",
            MeetingProvider::Meet => "meet",
            MeetingProvider::Webex => "webex",
            MeetingProvider::Jitsi => "jitsi",
            MeetingProvider::Other => "other",
        }
    }

    /// Inverse of [`as_str`](Self::as_str); an unknown token is `Other`.
    pub fn from_token(token: &str) -> MeetingProvider {
        match token {
            "teams" => MeetingProvider::Teams,
            "zoom" => MeetingProvider::Zoom,
            "meet" => MeetingProvider::Meet,
            "webex" => MeetingProvider::Webex,
            "jitsi" => MeetingProvider::Jitsi,
            _ => MeetingProvider::Other,
        }
    }

    /// The provider a join link belongs to, if any rule knows its host.
    pub fn of_url(url: &str) -> Option<MeetingProvider> {
        let host = host_of(url)?;
        let is = |domain: &str| host == domain || host.ends_with(&format!(".{domain}"));
        if is("teams.microsoft.com") || is("teams.live.com") {
            Some(MeetingProvider::Teams)
        } else if is("zoom.us") || is("zoom.com") {
            Some(MeetingProvider::Zoom)
        } else if is("meet.google.com") {
            Some(MeetingProvider::Meet)
        } else if is("webex.com") {
            Some(MeetingProvider::Webex)
        } else if is("meet.jit.si") || is("8x8.vc") {
            Some(MeetingProvider::Jitsi)
        } else {
            None
        }
    }
}

/// An event's online meeting: where it is held and, when known, the link that
/// joins it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct OnlineMeeting {
    pub provider: MeetingProvider,
    pub join_url: Option<String>,
}

impl OnlineMeeting {
    /// A meeting known by its join link alone — the provider follows from the
    /// host, `Other` when no rule knows it.
    pub fn from_join_url(url: &str) -> OnlineMeeting {
        OnlineMeeting {
            provider: MeetingProvider::of_url(url).unwrap_or(MeetingProvider::Other),
            join_url: Some(url.to_string()),
        }
    }
}

/// Phrases a calendar writes into an online meeting's location when it has no
/// link to put there ("Microsoft Teams Meeting", and its translations).
const TEAMS_PHRASES: &[&str] = &["microsoft teams"];

/// Find an online meeting in an event's free text — the location first, then
/// the body. A link a rule knows wins; failing that, a location that names
/// Teams makes a Teams meeting without a link.
pub fn detect(location: Option<&str>, body: Option<&str>) -> Option<OnlineMeeting> {
    let texts = [location, body];
    for text in texts.into_iter().flatten() {
        for url in urls_in(text) {
            if let Some(provider) = MeetingProvider::of_url(url) {
                return Some(OnlineMeeting {
                    provider,
                    join_url: Some(url.to_string()),
                });
            }
        }
    }
    let location = location?.to_lowercase();
    TEAMS_PHRASES
        .iter()
        .any(|phrase| location.contains(phrase))
        .then_some(OnlineMeeting {
            provider: MeetingProvider::Teams,
            join_url: None,
        })
}

/// The `http(s)://` links in `text`, each cut at whitespace or a closing
/// bracket/quote a link in prose is wrapped in.
fn urls_in(text: &str) -> impl Iterator<Item = &str> {
    let mut rest = text;
    std::iter::from_fn(move || {
        let at = ["https://", "http://"]
            .iter()
            .filter_map(|scheme| rest.find(scheme))
            .min()?;
        let tail = &rest[at..];
        let len = tail
            .find(|c: char| c.is_whitespace() || matches!(c, '>' | ')' | ']' | '"' | '\'' | '<'))
            .unwrap_or(tail.len());
        rest = &tail[len..];
        Some(tail[..len].trim_end_matches(['.', ',', ';']))
    })
}

/// The lowercase host of an `http(s)` URL.
fn host_of(url: &str) -> Option<String> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority.rsplit('@').next()?.split(':').next()?;
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn providers_follow_the_host() {
        let of = MeetingProvider::of_url;
        assert_eq!(
            of("https://teams.microsoft.com/l/meetup-join/19%3ameeting_x"),
            Some(MeetingProvider::Teams)
        );
        assert_eq!(of("https://example.zoom.us/j/123"), Some(MeetingProvider::Zoom));
        assert_eq!(of("https://meet.google.com/abc-defg-hij"), Some(MeetingProvider::Meet));
        assert_eq!(of("https://acme.webex.com/meet/x"), Some(MeetingProvider::Webex));
        assert_eq!(of("https://meet.jit.si/Room"), Some(MeetingProvider::Jitsi));
        assert_eq!(of("https://notzoom.us/j/1"), None);
        assert_eq!(of("https://example.invalid/teams.microsoft.com"), None);
    }

    #[test]
    fn tokens_round_trip() {
        for p in [
            MeetingProvider::Teams,
            MeetingProvider::Zoom,
            MeetingProvider::Meet,
            MeetingProvider::Webex,
            MeetingProvider::Jitsi,
            MeetingProvider::Other,
        ] {
            assert_eq!(MeetingProvider::from_token(p.as_str()), p);
        }
        assert_eq!(MeetingProvider::from_token("carrier-pigeon"), MeetingProvider::Other);
    }

    #[test]
    fn a_known_link_in_the_body_is_found() {
        let body = "Agenda first.\nJoin: <https://teams.microsoft.com/l/meetup-join/abc>.\nThanks";
        assert_eq!(
            detect(Some("Room 1"), Some(body)),
            Some(OnlineMeeting {
                provider: MeetingProvider::Teams,
                join_url: Some("https://teams.microsoft.com/l/meetup-join/abc".into()),
            })
        );
    }

    #[test]
    fn the_location_link_wins_over_the_body() {
        let found = detect(
            Some("https://example.zoom.us/j/42"),
            Some("fallback https://meet.google.com/x"),
        )
        .unwrap();
        assert_eq!(found.provider, MeetingProvider::Zoom);
    }

    #[test]
    fn unknown_links_are_not_meetings() {
        assert_eq!(detect(None, Some("see https://example.invalid/doc")), None);
    }

    #[test]
    fn a_teams_location_without_link_is_a_teams_meeting() {
        assert_eq!(
            detect(Some("Microsoft Teams Meeting"), Some("no link here")),
            Some(OnlineMeeting {
                provider: MeetingProvider::Teams,
                join_url: None,
            })
        );
        assert_eq!(detect(Some("Room 1"), None), None);
    }

    #[test]
    fn a_bare_join_url_is_classified() {
        assert_eq!(
            OnlineMeeting::from_join_url("https://example.invalid/call").provider,
            MeetingProvider::Other
        );
    }
}
