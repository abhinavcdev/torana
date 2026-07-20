use crate::config::RouteConfig;

/// Pick the first route whose host and path constraints match the request.
/// Config order is match priority: put more specific routes first. A route
/// with no `host`/`path` set matches anything, so it acts as a catch-all if
/// placed last.
pub fn select_route<'a>(
    routes: &'a [RouteConfig],
    host: Option<&str>,
    path: &str,
) -> Option<&'a RouteConfig> {
    routes
        .iter()
        .find(|r| host_matches(r.host.as_deref(), host) && path_matches(r.path.as_deref(), path))
}

fn host_matches(pattern: Option<&str>, actual: Option<&str>) -> bool {
    let Some(pattern) = pattern else {
        return true;
    };
    let Some(actual) = actual else {
        return false;
    };
    // Host headers may carry a port (`example.com:8080`); match on the
    // hostname only.
    let actual = actual.split(':').next().unwrap_or(actual);
    if let Some(suffix) = pattern.strip_prefix("*.") {
        actual.eq_ignore_ascii_case(suffix)
            || actual
                .to_ascii_lowercase()
                .ends_with(&format!(".{}", suffix.to_ascii_lowercase()))
    } else {
        actual.eq_ignore_ascii_case(pattern)
    }
}

fn path_matches(pattern: Option<&str>, actual: &str) -> bool {
    let Some(pattern) = pattern else {
        return true;
    };
    let pattern = pattern.trim_end_matches('/');
    if pattern.is_empty() {
        return true; // pattern was "/"
    }
    if !actual.starts_with(pattern) {
        return false;
    }
    // Require a segment boundary so "/api" doesn't match "/apiary".
    matches!(actual.as_bytes().get(pattern.len()), None | Some(b'/'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::UpstreamConfig;

    fn route(name: &str, host: Option<&str>, path: Option<&str>) -> RouteConfig {
        RouteConfig {
            name: name.to_string(),
            host: host.map(String::from),
            path: path.map(String::from),
            when: None,
            upstream: vec![UpstreamConfig {
                addr: "http://127.0.0.1:9000".into(),
                weight: None,
            }],
            mirror: None,
            timeout: Default::default(),
            headers: Default::default(),
            health_check: None,
            retries: None,
            plugin: None,
        }
    }

    #[test]
    fn matches_catch_all_when_no_constraints() {
        let routes = vec![route("default", None, None)];
        assert_eq!(
            select_route(&routes, Some("example.com"), "/anything")
                .unwrap()
                .name,
            "default"
        );
        assert_eq!(select_route(&routes, None, "/").unwrap().name, "default");
    }

    #[test]
    fn matches_exact_host() {
        let routes = vec![route("api", Some("api.example.com"), None)];
        assert!(select_route(&routes, Some("api.example.com"), "/").is_some());
        assert!(select_route(&routes, Some("api.example.com:8080"), "/").is_some());
        assert!(select_route(&routes, Some("other.example.com"), "/").is_none());
        assert!(select_route(&routes, None, "/").is_none());
    }

    #[test]
    fn matches_wildcard_host() {
        let routes = vec![route("tenant", Some("*.example.com"), None)];
        assert!(select_route(&routes, Some("a.example.com"), "/").is_some());
        assert!(select_route(&routes, Some("example.com"), "/").is_some());
        assert!(select_route(&routes, Some("notexample.com"), "/").is_none());
    }

    #[test]
    fn matches_path_prefix_on_segment_boundary() {
        let routes = vec![route("api", None, Some("/api"))];
        assert!(select_route(&routes, None, "/api").is_some());
        assert!(select_route(&routes, None, "/api/v1/users").is_some());
        assert!(select_route(&routes, None, "/apiary").is_none());
        assert!(select_route(&routes, None, "/").is_none());
    }

    #[test]
    fn first_match_wins_in_config_order() {
        let routes = vec![
            route("specific", Some("api.example.com"), Some("/v1")),
            route("catch-all", None, None),
        ];
        assert_eq!(
            select_route(&routes, Some("api.example.com"), "/v1/x")
                .unwrap()
                .name,
            "specific"
        );
        assert_eq!(
            select_route(&routes, Some("other.com"), "/v1/x")
                .unwrap()
                .name,
            "catch-all"
        );
    }

    #[test]
    fn no_match_returns_none() {
        let routes = vec![route("api", Some("api.example.com"), Some("/v1"))];
        assert!(select_route(&routes, Some("api.example.com"), "/v2").is_none());
    }
}
