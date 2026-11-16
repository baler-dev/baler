use baler_core::cran::client::git::{is_github_api_url, token_for};
use std::sync::Mutex;

// GITHUB_TOKEN and GH_TOKEN are process-global, and cargo runs the tests
// of one file on several threads, so every test that sets them holds
// this lock for its full duration.
static ENV_LOCK: Mutex<()> = Mutex::new(());

const TOKEN_VARS: [&str; 2] = ["GITHUB_TOKEN", "GH_TOKEN"];

/// Sets the given variables (and clears the other one) for the life of
/// the guard, then puts back what was there before.
struct TokenEnv {
    previous: Vec<(&'static str, Option<String>)>,
}

impl TokenEnv {
    fn set(github_token: Option<&str>, gh_token: Option<&str>) -> Self {
        let previous = TOKEN_VARS
            .iter()
            .map(|name| (*name, std::env::var(name).ok()))
            .collect();
        for (name, value) in TOKEN_VARS.iter().zip([github_token, gh_token]) {
            match value {
                Some(value) => unsafe { std::env::set_var(name, value) },
                None => unsafe { std::env::remove_var(name) },
            }
        }
        Self { previous }
    }
}

impl Drop for TokenEnv {
    fn drop(&mut self) {
        for (name, value) in &self.previous {
            match value {
                Some(value) => unsafe { std::env::set_var(name, value) },
                None => unsafe { std::env::remove_var(name) },
            }
        }
    }
}

#[test]
fn github_api_urls_are_recognised() {
    assert!(is_github_api_url("https://api.github.com/repos/user/repo/tarball"));
    assert!(is_github_api_url("https://api.github.com/repos/user/repo/tarball/v1.0.0"));
    assert!(is_github_api_url("https://API.GITHUB.COM/repos/user/repo/tarball"));
}

#[test]
fn other_hosts_are_never_github_api_urls() {
    let not_github = [
        "https://example.com/module_1.0.0.tar.gz",
        "https://github.com/user/repo/archive/main.tar.gz",
        "https://codeload.github.com/user/repo/legacy.tar.gz/main",
        "https://github.com/user/repo/releases/download/v1/tools_1.0.0.tar.gz",
        "https://raw.githubusercontent.com/user/repo/main/baler.toml",
    ];
    for url in not_github {
        assert!(!is_github_api_url(url), "{url} must not count as GitHub's API");
    }
}

#[test]
fn look_alike_urls_are_not_github_api_urls() {
    let look_alikes = [
        "https://api.github.com.example.org/repos/user/repo/tarball",
        "https://example.org/api.github.com/repos/user/repo/tarball",
        "https://example.org/?next=https://api.github.com/repos",
        "https://api.github.com@example.org/repos/user/repo/tarball",
        "https://user:pass@api.github.com/repos/user/repo/tarball",
        "https://notapi.github.com/repos/user/repo/tarball",
        "https://evil-api.github.com/repos/user/repo/tarball",
    ];
    for url in look_alikes {
        assert!(!is_github_api_url(url), "{url} must not count as GitHub's API");
    }
}

#[test]
fn plain_http_and_unparsable_urls_are_not_github_api_urls() {
    assert!(!is_github_api_url("http://api.github.com/repos/user/repo/tarball"));
    assert!(!is_github_api_url("ftp://api.github.com/repos/user/repo/tarball"));
    assert!(!is_github_api_url("api.github.com/repos/user/repo/tarball"));
    assert!(!is_github_api_url(""));
    assert!(!is_github_api_url("not a url"));
}

#[test]
fn token_goes_to_the_github_api() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let _env = TokenEnv::set(Some("secret-github-token"), None);

    assert_eq!(
        token_for("https://api.github.com/repos/user/repo/tarball").as_deref(),
        Some("secret-github-token")
    );
}

#[test]
fn gh_token_is_used_when_github_token_is_unset() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let _env = TokenEnv::set(None, Some("secret-gh-token"));

    assert_eq!(
        token_for("https://api.github.com/repos/user/repo/tarball").as_deref(),
        Some("secret-gh-token")
    );
}

#[test]
fn github_token_wins_over_gh_token() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let _env = TokenEnv::set(Some("first"), Some("second"));

    assert_eq!(
        token_for("https://api.github.com/repos/user/repo/tarball").as_deref(),
        Some("first")
    );
}

#[test]
fn token_never_goes_to_another_host() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let _env = TokenEnv::set(Some("secret-github-token"), Some("secret-gh-token"));

    let other_urls = [
        "https://example.com/module_1.0.0.tar.gz",
        "https://api.github.com.example.org/repos/user/repo/tarball",
        "https://api.github.com@example.org/repos/user/repo/tarball",
        "http://api.github.com/repos/user/repo/tarball",
    ];
    for url in other_urls {
        assert_eq!(token_for(url), None, "the token must not be sent to {url}");
    }
}

#[test]
fn empty_token_is_treated_as_unset() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let _env = TokenEnv::set(Some(""), None);

    assert_eq!(token_for("https://api.github.com/repos/user/repo/tarball"), None);
}

#[test]
fn no_token_set_means_no_token_sent() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let _env = TokenEnv::set(None, None);

    assert_eq!(token_for("https://api.github.com/repos/user/repo/tarball"), None);
}
