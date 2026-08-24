// YouTube video-ID parsing (spec §10).
//
// The whole point is to parse by HOST and PATH, never by searching the string.
// A substring search for "youtube.com/watch?v=" matches notyoutube.com/watch?v=…
// and an article whose URL contains a linked video id — both would wrongly start a
// paid transcription session. So we parse the URL structurally and validate the host
// against an allow-list of real YouTube hostnames.

const YT_HOSTS = new Set([
  "youtube.com",
  "www.youtube.com",
  "m.youtube.com",
  "music.youtube.com",
  "gaming.youtube.com",
]);

const YT_SHORT_HOSTS = new Set(["youtu.be", "www.youtu.be"]);

// A YouTube video id is exactly 11 chars of [A-Za-z0-9_-]. Validating the shape
// keeps a stray ?v=... on a real YouTube host (e.g. a playlist page) from being
// treated as a video when the value isn't actually an id.
const VIDEO_ID_RE = /^[A-Za-z0-9_-]{11}$/;

export interface YouTubeVideo {
  videoId: string;
  /** A canonical URL rebuilt from the id — NOT the address bar's tracking params. */
  canonicalUrl: string;
}

function hostMatches(host: string, allowed: Set<string>): boolean {
  return allowed.has(host.toLowerCase());
}

/**
 * Extract a YouTube video id from a URL, or null if the URL is not a YouTube video.
 * Covers watch, youtu.be, shorts, live, embed. Finds `v` anywhere in the query.
 */
export function parseYouTube(rawUrl: string | null | undefined): YouTubeVideo | null {
  if (!rawUrl || typeof rawUrl !== "string") return null;

  let url: URL;
  try {
    url = new URL(rawUrl.trim());
  } catch {
    return null; // junk input — not a URL at all
  }

  if (url.protocol !== "http:" && url.protocol !== "https:") return null;

  const host = url.hostname.toLowerCase();

  // youtu.be/<id> — the id is the first path segment.
  if (hostMatches(host, YT_SHORT_HOSTS)) {
    const seg = firstSegment(url.pathname);
    return finish(seg);
  }

  if (!hostMatches(host, YT_HOSTS)) return null; // rejects notyoutube.com etc.

  // /watch?v=<id> — v may appear anywhere in the query (after other params).
  const segments = url.pathname.split("/").filter(Boolean);
  const kind = segments[0]?.toLowerCase();

  if (url.pathname === "/watch" || kind === "watch") {
    return finish(url.searchParams.get("v"));
  }

  // /shorts/<id>, /live/<id>, /embed/<id>
  if (kind === "shorts" || kind === "live" || kind === "embed") {
    return finish(segments[1] ?? null);
  }

  // Some watch URLs still carry ?v= without a /watch path (rare, but harmless to
  // accept as long as the id validates against the strict shape).
  const v = url.searchParams.get("v");
  if (v) return finish(v);

  return null;
}

function firstSegment(pathname: string): string | null {
  const seg = pathname.split("/").filter(Boolean)[0];
  return seg ?? null;
}

function finish(id: string | null): YouTubeVideo | null {
  if (!id) return null;
  if (!VIDEO_ID_RE.test(id)) return null;
  return {
    videoId: id,
    // Rebuild canonically — drops utm_*, si=, t=, feature=, etc.
    canonicalUrl: `https://www.youtube.com/watch?v=${id}`,
  };
}
