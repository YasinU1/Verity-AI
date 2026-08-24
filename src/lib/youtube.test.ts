import { describe, it, expect } from "vitest";
import { parseYouTube } from "./youtube";

const ID = "dQw4w9WgXcQ"; // 11-char valid id
const CANON = `https://www.youtube.com/watch?v=${ID}`;

describe("parseYouTube — every YouTube shape", () => {
  it("standard watch URL", () => {
    expect(parseYouTube(`https://www.youtube.com/watch?v=${ID}`)?.videoId).toBe(ID);
  });

  it("youtu.be short link", () => {
    expect(parseYouTube(`https://youtu.be/${ID}`)?.videoId).toBe(ID);
  });

  it("shorts", () => {
    expect(parseYouTube(`https://www.youtube.com/shorts/${ID}`)?.videoId).toBe(ID);
  });

  it("live", () => {
    expect(parseYouTube(`https://www.youtube.com/live/${ID}`)?.videoId).toBe(ID);
  });

  it("embed", () => {
    expect(parseYouTube(`https://www.youtube.com/embed/${ID}`)?.videoId).toBe(ID);
  });

  it("mobile host", () => {
    expect(parseYouTube(`https://m.youtube.com/watch?v=${ID}`)?.videoId).toBe(ID);
  });

  it("music host", () => {
    expect(parseYouTube(`https://music.youtube.com/watch?v=${ID}`)?.videoId).toBe(ID);
  });

  it("finds v anywhere in the query (after list, before t)", () => {
    const out = parseYouTube(`https://www.youtube.com/watch?list=PLxyz&v=${ID}&t=42s`);
    expect(out?.videoId).toBe(ID);
  });
});

describe("parseYouTube — canonicalization drops tracking params", () => {
  it("rebuilds a clean canonical URL from watch", () => {
    expect(parseYouTube(`https://www.youtube.com/watch?v=${ID}&t=90&si=abc&utm_source=x`)?.canonicalUrl).toBe(CANON);
  });

  it("rebuilds a clean canonical URL from youtu.be with ?si=", () => {
    expect(parseYouTube(`https://youtu.be/${ID}?si=trackme`)?.canonicalUrl).toBe(CANON);
  });
});

describe("parseYouTube — rejects impostors and junk", () => {
  it("rejects notyoutube.com", () => {
    expect(parseYouTube(`https://notyoutube.com/watch?v=${ID}`)).toBeNull();
  });

  it("rejects a third-party host whose path merely mentions youtube.com", () => {
    expect(parseYouTube(`https://example.com/article-about-youtube.com/watch?v=${ID}`)).toBeNull();
  });

  it("rejects an article whose text contains a video URL as a substring", () => {
    // The address bar is on a news site — the video URL is only inside the path.
    expect(parseYouTube(`https://news.example.com/story/youtube.com-watch-v-${ID}`)).toBeNull();
  });

  it("rejects non-http protocols", () => {
    expect(parseYouTube(`ftp://youtube.com/watch?v=${ID}`)).toBeNull();
  });

  it("rejects an id of the wrong length", () => {
    expect(parseYouTube("https://www.youtube.com/watch?v=short")).toBeNull();
  });

  it("rejects a watch URL with no v param", () => {
    expect(parseYouTube("https://www.youtube.com/watch?list=PLxyz")).toBeNull();
  });

  it("rejects plain junk", () => {
    expect(parseYouTube("not a url at all")).toBeNull();
  });

  it("rejects empty / null / undefined", () => {
    expect(parseYouTube("")).toBeNull();
    expect(parseYouTube(null)).toBeNull();
    expect(parseYouTube(undefined)).toBeNull();
  });
});
