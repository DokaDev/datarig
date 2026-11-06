use super::*;

/// `pairs` as the environment: upper case names, then lower case ones.
fn proxy(pairs: &[(&str, &str)], host: &str) -> Proxy {
    let get = |name: &str| {
        let lower = name.to_ascii_lowercase();
        let find = |n: &str| pairs.iter().find(|(k, _)| *k == n).map(|(_, v)| v.to_string());
        find(name).filter(|v| !v.is_empty()).or_else(|| find(&lower))
    };
    for_host(&Vars { https_proxy: get("HTTPS_PROXY"), all_proxy: get("ALL_PROXY"), no_proxy: get("NO_PROXY") }, host)
}

const H: &str = "api.github.com";

fn http(s: &str) -> Proxy {
    Proxy::Http(s.to_string())
}

#[test]
fn no_proxy_set_is_direct() {
    assert_eq!(proxy(&[], H), Proxy::Direct);
    assert_eq!(proxy(&[("HTTPS_PROXY", ""), ("ALL_PROXY", "  ")], H), Proxy::Direct);
}

#[test]
fn https_proxy_then_all_proxy_upper_case_first() {
    assert_eq!(proxy(&[("HTTPS_PROXY", "http://a:3128")], H), http("http://a:3128"));
    assert_eq!(proxy(&[("https_proxy", "http://b:3128")], H), http("http://b:3128"));
    assert_eq!(proxy(&[("HTTPS_PROXY", "http://a:1"), ("https_proxy", "http://b:2")], H), http("http://a:1"));
    assert_eq!(proxy(&[("ALL_PROXY", "http://c:3")], H), http("http://c:3"));
    assert_eq!(proxy(&[("all_proxy", "http://d:4")], H), http("http://d:4"));
    assert_eq!(proxy(&[("all_proxy", "http://d:4"), ("https_proxy", "http://b:2")], H), http("http://b:2"));
}

#[test]
fn an_http_proxy_with_or_without_its_scheme_and_credentials() {
    for (set, want) in [
        ("http://proxy.corp:8080", "http://proxy.corp:8080"),
        ("HTTP://proxy.corp:8080/", "http://proxy.corp:8080"),
        ("proxy.corp:8080", "http://proxy.corp:8080"),
        ("proxy.corp", "http://proxy.corp"),
        ("http://user:p%40ss@proxy.corp:8080", "http://user:p%40ss@proxy.corp:8080"),
        ("http://[::1]:3128", "http://[::1]:3128"),
        ("http://[::1]", "http://[::1]"),
        ("http://10.0.0.1:3128", "http://10.0.0.1:3128"),
    ] {
        assert_eq!(proxy(&[("HTTPS_PROXY", set)], H), http(want), "{set}");
    }
}

#[test]
fn any_other_proxy_means_no_check_never_a_direct_connection() {
    for set in [
        "socks5://proxy:1080",
        "socks5h://proxy:1080",
        "socks4://proxy:1080",
        "https://proxy:443",
        "ftp://proxy",
        "http://",
        "http://:8080",
        "http://proxy:port",
        "http://proxy:99999",
        "http://proxy:",
        "http://proxy:8080/path",
        "http://pro xy:8080",
        "http://[::1",
        "http://[::1]x",
    ] {
        assert_eq!(proxy(&[("HTTPS_PROXY", set)], H), Proxy::Unsupported, "{set}");
    }
}

#[test]
fn no_proxy_matches_the_host_and_its_parents() {
    let p = |list: &str| proxy(&[("HTTPS_PROXY", "http://p:1"), ("NO_PROXY", list)], H);
    for direct in [
        "api.github.com",
        "github.com",
        ".github.com",
        "*.github.com",
        "*",
        "localhost, github.com",
        "GitHub.com",
        "github.com:443",
        " x ,api.github.com ",
    ] {
        assert_eq!(p(direct), Proxy::Direct, "{direct}");
    }
    for through in ["hub.com", "ithub.com", "api.github.com.evil", "example.com", "", ",", "."] {
        assert_eq!(p(through), http("http://p:1"), "{through}");
    }
    assert_eq!(proxy(&[("HTTPS_PROXY", "http://p:1"), ("no_proxy", "github.com")], H), Proxy::Direct);
    // An unusable proxy for a host it does not cover is no matter.
    assert_eq!(proxy(&[("HTTPS_PROXY", "socks5://p:1"), ("NO_PROXY", "github.com")], H), Proxy::Direct);
}
