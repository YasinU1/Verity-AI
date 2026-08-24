//! Auto-start (spec §10). Poll the frontmost app; when it's a browser on a YouTube
//! video, offer to start a session. The two things that make this not-annoying:
//!   * Start on the VIDEO, not the browser. A browser is frontmost most of the working
//!     day; starting a paid, per-minute transcription session against a spreadsheet tab
//!     is exactly the wrong behaviour. Require an actual video when the URL is readable.
//!   * Parse YouTube URLs by host+path, never by substring — otherwise
//!     notyoutube.com/watch?v=… and an article mentioning a video both register.

use serde::Serialize;
use url::Url;

// --- YouTube parsing (pure, unit-tested; mirrors the frontend parser) ---

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct YouTubeVideo {
    pub video_id: String,
    /// A canonical URL rebuilt from the id — NOT the address bar's tracking params.
    pub canonical_url: String,
}

const YT_HOSTS: &[&str] = &[
    "youtube.com",
    "www.youtube.com",
    "m.youtube.com",
    "music.youtube.com",
    "gaming.youtube.com",
];
const YT_SHORT_HOSTS: &[&str] = &["youtu.be", "www.youtu.be"];

fn valid_video_id(id: &str) -> bool {
    id.len() == 11 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn finish(id: Option<&str>) -> Option<YouTubeVideo> {
    let id = id?;
    if !valid_video_id(id) {
        return None;
    }
    Some(YouTubeVideo {
        video_id: id.to_string(),
        canonical_url: format!("https://www.youtube.com/watch?v={id}"),
    })
}

/// Extract a YouTube video from a URL, or None. Covers watch, youtu.be, shorts, live,
/// embed; finds `v` anywhere in the query.
pub fn parse_youtube(raw: &str) -> Option<YouTubeVideo> {
    let url = Url::parse(raw.trim()).ok()?;
    if url.scheme() != "http" && url.scheme() != "https" {
        return None;
    }
    let host = url.host_str()?.to_lowercase();

    // youtu.be/<id>
    if YT_SHORT_HOSTS.contains(&host.as_str()) {
        let seg = url.path_segments().and_then(|mut s| s.next());
        return finish(seg);
    }

    if !YT_HOSTS.contains(&host.as_str()) {
        return None; // rejects notyoutube.com and third-party hosts
    }

    let segments: Vec<&str> = url
        .path_segments()
        .map(|s| s.filter(|x| !x.is_empty()).collect())
        .unwrap_or_default();
    let kind = segments.first().copied().unwrap_or("").to_lowercase();

    if url.path() == "/watch" || kind == "watch" {
        let v = url.query_pairs().find(|(k, _)| k == "v").map(|(_, v)| v.to_string());
        return finish(v.as_deref());
    }
    if kind == "shorts" || kind == "live" || kind == "embed" {
        return finish(segments.get(1).copied());
    }
    // Fallback: a ?v= on a real YouTube host, id validated by shape.
    let v = url.query_pairs().find(|(k, _)| k == "v").map(|(_, v)| v.to_string());
    finish(v.as_deref())
}

// --- Browser classification by BUNDLE ID (names are localised and renameable) ---

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum BrowserKind {
    Chromium,
    Safari,
    Firefox,
    NotBrowser,
}

pub fn browser_kind(bundle_id: &str) -> BrowserKind {
    match bundle_id {
        "com.google.Chrome"
        | "com.google.Chrome.canary"
        | "com.brave.Browser"
        | "com.microsoft.edgemac"
        | "com.vivaldi.Vivaldi"
        | "com.operasoftware.Opera"
        | "company.thebrowser.Browser" // Arc
        | "com.google.Chrome.beta" => BrowserKind::Chromium,
        "com.apple.Safari" | "com.apple.SafariTechnologyPreview" => BrowserKind::Safari,
        "org.mozilla.firefox" | "org.mozilla.firefoxdeveloperedition" => BrowserKind::Firefox,
        _ => BrowserKind::NotBrowser,
    }
}

/// The AppleScript to read the active tab URL, targeted by bundle id. Wrapped in a 2s
/// timeout because osascript blocks until the browser answers, and a browser loading a
/// heavy page would otherwise stall the poll loop and pile up processes.
/// Firefox has no usable tab dictionary — we report it, never ask.
pub fn active_tab_script(kind: BrowserKind, bundle_id: &str) -> Option<String> {
    let inner = match kind {
        BrowserKind::Chromium => "URL of active tab of front window",
        BrowserKind::Safari => "URL of current tab of front window",
        BrowserKind::Firefox | BrowserKind::NotBrowser => return None,
    };
    Some(format!(
        "with timeout of 2 seconds\n  tell application id \"{bundle_id}\" to return {inner}\nend timeout"
    ))
}

// --- Status the UI can act on. Collapsing these to null would hide the one-click fix
//     that permission_denied actually has. ---

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum AutoStartStatus {
    /// A readable browser tab; `video` is Some when it's a startable YouTube video.
    Ok {
        bundle_id: String,
        url: String,
        video: Option<YouTubeVideo>,
    },
    /// Frontmost app isn't a browser.
    NotBrowser { bundle_id: String, name: String },
    /// Frontmost is a browser we can't read tabs from (Firefox).
    Unsupported { bundle_id: String },
    /// Automation permission not granted — a one-click fix the UI can prompt for.
    PermissionDenied { bundle_id: String },
    /// No frontmost app / not on macOS.
    Unavailable,
}

/// Classify an osascript outcome into a status. `osa_error` is the tool's stderr, if any.
pub fn classify_osascript(
    bundle_id: &str,
    kind: BrowserKind,
    url: Option<String>,
    osa_error: Option<&str>,
) -> AutoStartStatus {
    if kind == BrowserKind::Firefox {
        return AutoStartStatus::Unsupported { bundle_id: bundle_id.to_string() };
    }
    if let Some(err) = osa_error {
        let e = err.to_lowercase();
        // -1743 / "Not authorized to send Apple events" is the permission case.
        if e.contains("not authorized") || e.contains("-1743") || e.contains("permission") {
            return AutoStartStatus::PermissionDenied { bundle_id: bundle_id.to_string() };
        }
    }
    match url {
        Some(u) if !u.trim().is_empty() => {
            let video = parse_youtube(&u);
            AutoStartStatus::Ok { bundle_id: bundle_id.to_string(), url: u, video }
        }
        _ => AutoStartStatus::PermissionDenied { bundle_id: bundle_id.to_string() },
    }
}

/// Given a frontmost app, should we auto-start — and against what?
///
/// Start on the video when the URL is readable. Fall back to any-browser when it isn't
/// (Firefox, permission denied): never firing there is indistinguishable from the
/// feature being broken, so a browser-frontmost fallback is better than silence.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum AutoStartDecision {
    StartVideo { canonical_url: String },
    StartBrowserFallback,
    DoNothing,
}

pub fn decide(status: &AutoStartStatus) -> AutoStartDecision {
    match status {
        AutoStartStatus::Ok { video: Some(v), .. } => {
            AutoStartDecision::StartVideo { canonical_url: v.canonical_url.clone() }
        }
        // A browser tab that isn't a video → do nothing (don't bill against a spreadsheet).
        AutoStartStatus::Ok { video: None, .. } => AutoStartDecision::DoNothing,
        // We know it's a browser but can't read the tab → fall back rather than never fire.
        AutoStartStatus::Unsupported { .. } | AutoStartStatus::PermissionDenied { .. } => {
            AutoStartDecision::StartBrowserFallback
        }
        _ => AutoStartDecision::DoNothing,
    }
}

// --- Frontmost app (macOS/AppKit). Returns None off-platform so the frontend degrades
//     rather than breaking. ---

#[derive(Debug, Clone, Serialize)]
pub struct ActiveApp {
    pub bundle_id: String,
    pub name: String,
}

#[cfg(all(target_os = "macos", feature = "appkit"))]
pub fn active_app() -> Option<ActiveApp> {
    use objc2_app_kit::NSWorkspace;
    // NSWorkspace is main-thread-safe for this read.
    let workspace = unsafe { NSWorkspace::sharedWorkspace() };
    let app = unsafe { workspace.frontmostApplication() }?;
    let bundle_id = unsafe { app.bundleIdentifier() }.map(|s| s.to_string()).unwrap_or_default();
    let name = unsafe { app.localizedName() }.map(|s| s.to_string()).unwrap_or_default();
    if bundle_id.is_empty() {
        return None;
    }
    Some(ActiveApp { bundle_id, name })
}

#[cfg(not(all(target_os = "macos", feature = "appkit")))]
pub fn active_app() -> Option<ActiveApp> {
    None
}

/// Run the tab-URL AppleScript for the frontmost browser. Only sends Apple events to
/// actual browsers — firing one at Zoom or Keynote raises a permission prompt for an app
/// with no tabs.
pub fn read_active_tab(bundle_id: &str) -> AutoStartStatus {
    let kind = browser_kind(bundle_id);
    if kind == BrowserKind::NotBrowser {
        return AutoStartStatus::NotBrowser {
            bundle_id: bundle_id.to_string(),
            name: bundle_id.to_string(),
        };
    }
    let Some(script) = active_tab_script(kind, bundle_id) else {
        return AutoStartStatus::Unsupported { bundle_id: bundle_id.to_string() };
    };
    let output = std::process::Command::new("osascript").arg("-e").arg(&script).output();
    match output {
        Ok(out) => {
            let url = String::from_utf8_lossy(&out.stdout).trim().to_string();
            let err = String::from_utf8_lossy(&out.stderr).to_string();
            classify_osascript(
                bundle_id,
                kind,
                if url.is_empty() { None } else { Some(url) },
                if err.is_empty() { None } else { Some(&err) },
            )
        }
        Err(_) => AutoStartStatus::Unavailable,
    }
}

/// Full poll: frontmost app → tab status.
pub fn poll() -> AutoStartStatus {
    match active_app() {
        Some(app) => read_active_tab(&app.bundle_id),
        None => AutoStartStatus::Unavailable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "dQw4w9WgXcQ";

    #[test]
    fn parses_every_youtube_shape() {
        assert_eq!(parse_youtube(&format!("https://www.youtube.com/watch?v={ID}")).unwrap().video_id, ID);
        assert_eq!(parse_youtube(&format!("https://youtu.be/{ID}")).unwrap().video_id, ID);
        assert_eq!(parse_youtube(&format!("https://www.youtube.com/shorts/{ID}")).unwrap().video_id, ID);
        assert_eq!(parse_youtube(&format!("https://www.youtube.com/live/{ID}")).unwrap().video_id, ID);
        assert_eq!(parse_youtube(&format!("https://www.youtube.com/embed/{ID}")).unwrap().video_id, ID);
        assert_eq!(parse_youtube(&format!("https://m.youtube.com/watch?v={ID}")).unwrap().video_id, ID);
    }

    #[test]
    fn finds_v_anywhere_in_the_query() {
        let out = parse_youtube(&format!("https://www.youtube.com/watch?list=PLxyz&v={ID}&t=42s")).unwrap();
        assert_eq!(out.video_id, ID);
    }

    #[test]
    fn rebuilds_a_canonical_url_dropping_tracking() {
        let out = parse_youtube(&format!("https://youtu.be/{ID}?si=trackme&t=90")).unwrap();
        assert_eq!(out.canonical_url, format!("https://www.youtube.com/watch?v={ID}"));
    }

    #[test]
    fn rejects_impostors_and_junk() {
        assert!(parse_youtube(&format!("https://notyoutube.com/watch?v={ID}")).is_none());
        assert!(parse_youtube(&format!("https://example.com/about-youtube.com/watch?v={ID}")).is_none());
        assert!(parse_youtube(&format!("https://news.site/story/youtube.com-watch-v-{ID}")).is_none());
        assert!(parse_youtube(&format!("ftp://youtube.com/watch?v={ID}")).is_none());
        assert!(parse_youtube("https://www.youtube.com/watch?v=short").is_none());
        assert!(parse_youtube("https://www.youtube.com/watch?list=PLxyz").is_none());
        assert!(parse_youtube("not a url").is_none());
        assert!(parse_youtube("").is_none());
    }

    #[test]
    fn classifies_browsers_by_bundle_id() {
        assert_eq!(browser_kind("com.google.Chrome"), BrowserKind::Chromium);
        assert_eq!(browser_kind("com.brave.Browser"), BrowserKind::Chromium);
        assert_eq!(browser_kind("company.thebrowser.Browser"), BrowserKind::Chromium);
        assert_eq!(browser_kind("com.apple.Safari"), BrowserKind::Safari);
        assert_eq!(browser_kind("org.mozilla.firefox"), BrowserKind::Firefox);
        assert_eq!(browser_kind("us.zoom.xos"), BrowserKind::NotBrowser);
    }

    #[test]
    fn firefox_has_no_tab_script() {
        assert!(active_tab_script(BrowserKind::Firefox, "org.mozilla.firefox").is_none());
        assert!(active_tab_script(BrowserKind::Chromium, "com.google.Chrome").unwrap().contains("active tab"));
        assert!(active_tab_script(BrowserKind::Safari, "com.apple.Safari").unwrap().contains("current tab"));
        // Every script is bounded by a 2s timeout.
        assert!(active_tab_script(BrowserKind::Chromium, "com.google.Chrome").unwrap().contains("with timeout of 2 seconds"));
    }

    #[test]
    fn permission_error_is_classified_distinctly() {
        let s = classify_osascript(
            "com.google.Chrome",
            BrowserKind::Chromium,
            None,
            Some("execution error: Not authorized to send Apple events (-1743)"),
        );
        assert_eq!(s, AutoStartStatus::PermissionDenied { bundle_id: "com.google.Chrome".into() });
    }

    #[test]
    fn firefox_classifies_as_unsupported_not_permission() {
        let s = classify_osascript("org.mozilla.firefox", BrowserKind::Firefox, None, None);
        assert_eq!(s, AutoStartStatus::Unsupported { bundle_id: "org.mozilla.firefox".into() });
    }

    // --- Live smoke tests. These depend on machine state and macOS Automation
    //     permission, so they don't belong in CI — but they're how you confirm the
    //     AppleScript path actually works. Run with: cargo test -- --ignored ---

    #[test]
    #[ignore = "live: needs a frontmost app on macOS"]
    fn smoke_frontmost_app() {
        let app = active_app();
        println!("frontmost app: {app:?}");
        assert!(app.is_some(), "expected a frontmost application");
    }

    #[test]
    #[ignore = "live: open a Chrome tab first; needs Automation permission"]
    fn smoke_chrome_tab_url() {
        let status = read_active_tab("com.google.Chrome");
        println!("chrome tab status: {status:?}");
        // Any non-Unavailable status proves the osascript path executed.
        assert_ne!(status, AutoStartStatus::Unavailable);
    }

    #[test]
    fn decision_starts_only_on_a_real_video() {
        // A YouTube video → start on the canonical URL.
        let ok_video = classify_osascript(
            "com.google.Chrome",
            BrowserKind::Chromium,
            Some(format!("https://www.youtube.com/watch?v={ID}&t=5")),
            None,
        );
        assert_eq!(
            decide(&ok_video),
            AutoStartDecision::StartVideo { canonical_url: format!("https://www.youtube.com/watch?v={ID}") }
        );

        // A non-video browser tab (a spreadsheet) → do nothing. This is the bill-saver.
        let ok_sheet = classify_osascript(
            "com.google.Chrome",
            BrowserKind::Chromium,
            Some("https://docs.google.com/spreadsheets/d/abc".into()),
            None,
        );
        assert_eq!(decide(&ok_sheet), AutoStartDecision::DoNothing);

        // Firefox / permission denied → fall back rather than never firing.
        assert_eq!(
            decide(&AutoStartStatus::Unsupported { bundle_id: "org.mozilla.firefox".into() }),
            AutoStartDecision::StartBrowserFallback
        );
    }

    #[test]
    fn decide_does_nothing_for_non_browser_or_unavailable() {
        assert_eq!(
            decide(&AutoStartStatus::NotBrowser { bundle_id: "us.zoom.xos".into(), name: "Zoom".into() }),
            AutoStartDecision::DoNothing
        );
        assert_eq!(decide(&AutoStartStatus::Unavailable), AutoStartDecision::DoNothing);
    }

    #[test]
    fn non_browser_frontmost_never_gets_an_apple_event() {
        // read_active_tab short-circuits for non-browsers (no osascript, no prompt).
        let s = read_active_tab("us.zoom.xos");
        assert!(matches!(s, AutoStartStatus::NotBrowser { .. }));
    }

    #[test]
    fn permission_denied_is_the_only_one_way_fixable_status() {
        // A blank URL with no error is treated as a permission problem the UI can fix.
        let s = classify_osascript("com.apple.Safari", BrowserKind::Safari, None, None);
        assert_eq!(s, AutoStartStatus::PermissionDenied { bundle_id: "com.apple.Safari".into() });
    }
}
