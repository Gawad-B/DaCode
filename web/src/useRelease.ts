
export type Os = "windows" | "linux" | "macos" | "unknown";

export type Download = {
  /** What the button says. */
  label: string;
  /** The asset filename, or a command for platforms with no artifact. */
  detail: string;
  url: string;
  size?: number;
};

export type Platform = {
  os: Os;
  /** The name to print. Not used to choose anything — `os` is. */
  name: string;
  /** null when there is no build for this platform. */
  primary: Download | null;
  /**
   * What to say in place of the button when there is nothing to download. A
   * label rather than a link: the source is private, so "build it yourself"
   * is no longer an offer this page can make.
   */
  pending: string | null;
  /** Other artifacts for the same platform: the rpm and the tarball on Linux. */
  alternatives: Download[];
  /** The line under the button. Honest about what the visitor is getting. */
  note: string;
  /** Shell prompt and command for the install box. */
  prompt: string;
  command: string;
};

export type Release = {
  version: string;
  published: string | null;
  assets: Record<string, { url: string; size: number }>;
  /** Kept so callers need not change; nothing is fetched, so nothing is stale. */
  stale: boolean;
};

/*
 * Downloads come from a release on Gawad-B/dacode-releases, a public repository
 * that holds nothing at all — one empty commit, so that a tag can exist, and
 * four assets hanging off it. The editor's own repository is private, which
 * took its releases private with it: the assets and the API that listed them
 * answer 404 to anyone not signed in as the owner, and the page could not see
 * that. It fetched the API, ignored the failure, and fell back to hard-coded
 * GitHub URLs — so every button rendered with the right version, date and size,
 * and 404'd on click, for everyone except the owner, whose session resolved
 * them.
 *
 * Google Drive was tried in between and is what this replaced. The bytes are
 * reachable, but which URL Drive honours depends on the visitor's session: the
 * download host answers with the file to a request carrying no cookies and with
 * the "Google Drive can't scan this file for viruses" page to a signed-in one.
 * A download that works when tested and not when clicked is worse than one that
 * plainly fails, and none of it is configurable from this end.
 *
 * The cost of doing it this way is that a new release must be uploaded there
 * and the version written in here. `conventions.rs` fails the build if this
 * table names a version the crate is not.
 */
const ASSETS = "https://github.com/Gawad-B/dacode-releases/releases/download";

const RELEASE: Release = {
  version: "1.0.0",
  published: "2026-10-27",
  assets: {
    "dacode-setup.exe": {
      url: `${ASSETS}/v1.0.0/dacode-setup.exe`,
      size: 7953718,
    },
    "dacode_1.0.0_amd64.deb": {
      url: `${ASSETS}/v1.0.0/dacode_1.0.0_amd64.deb`,
      size: 9849304,
    },
    "dacode-1.0.0-1.x86_64.rpm": {
      url: `${ASSETS}/v1.0.0/dacode-1.0.0-1.x86_64.rpm`,
      size: 9748746,
    },
    "dacode-1.0.0-x86_64-linux.tar.gz": {
      url: `${ASSETS}/v1.0.0/dacode-1.0.0-x86_64-linux.tar.gz`,
      size: 9848676,
    },
  },
  stale: false,
};

export function useRelease(): Release {
  return RELEASE;
}

/*
 * Detection reads the platform, never the browser. `userAgentData.platform` is
 * the one that is not a compatibility string; the userAgent is the fallback for
 * Firefox and Safari, which do not have it. A phone is deliberately "unknown" —
 * there is no build for it and pretending otherwise hands someone a 9 MB file
 * they cannot run.
 */
export function detectOs(): Os {
  if (typeof navigator === "undefined") return "unknown";

  const uaData = (navigator as Navigator & { userAgentData?: { platform?: string } })
    .userAgentData;
  const platform = (uaData?.platform || navigator.platform || "").toLowerCase();
  const ua = navigator.userAgent.toLowerCase();

  if (/android|iphone|ipad|ipod/.test(ua)) return "unknown";
  // iPadOS reports itself as a Mac; it has a touch point count and no build.
  if (/mac/.test(platform) && navigator.maxTouchPoints > 1) return "unknown";

  if (/win/.test(platform) || /windows/.test(ua)) return "windows";
  if (/mac|darwin/.test(platform) || /mac os x/.test(ua)) return "macos";
  if (/linux|x11|cros|bsd/.test(platform) || /linux|x11|cros|bsd/.test(ua)) return "linux";

  return "unknown";
}

export function formatSize(bytes?: number): string {
  if (!bytes) return "";
  return `${(bytes / 1_000_000).toFixed(1)} MB`;
}

export function formatDate(iso: string | null): string {
  if (!iso) return "";
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "";
  return d.toLocaleDateString(undefined, { day: "numeric", month: "short", year: "numeric" });
}

/** Drops the artifacts a release does not carry, keeping the rest typed. */
function present(list: (Download | null)[]): Download[] {
  return list.filter((a): a is Download => a !== null);
}

/** Matches an asset by shape rather than by exact name, since the version moves. */
function find(release: Release, test: (name: string) => boolean) {
  const name = Object.keys(release.assets).find(test);
  return name ? { name, ...release.assets[name] } : null;
}

export function platformFor(os: Os, release: Release): Platform {
  const exe = find(release, (n) => n.endsWith(".exe"));
  const deb = find(release, (n) => n.endsWith(".deb"));
  const rpm = find(release, (n) => n.endsWith(".rpm"));
  const tar = find(release, (n) => n.endsWith(".tar.gz"));

  switch (os) {
    case "windows":
      return {
        os,
        name: "Windows",
        primary: exe && {
          label: "Download for Windows",
          detail: exe.name,
          url: exe.url,
          size: exe.size,
        },
        pending: null,
        alternatives: [],
        note: "Per-user install, no admin prompt. Start Menu, PATH, “Open with DaCode”, uninstaller.",
        prompt: ">",
        command: ".\\dacode-setup.exe",
      };

    case "linux":
      return {
        os,
        name: "Linux",
        primary: deb && {
          label: "Download for Linux",
          detail: deb.name,
          url: deb.url,
          size: deb.size,
        },
        pending: null,
        alternatives: present([
          rpm && { label: ".rpm — Fedora, RHEL, openSUSE", detail: rpm.name, url: rpm.url, size: rpm.size },
          tar && { label: ".tar.gz — any distribution", detail: tar.name, url: tar.url, size: tar.size },
        ]),
        note: "Debian, Ubuntu, Mint. The tarball installs into ~/.local with no root.",
        prompt: "$",
        command: deb ? `sudo apt install ./${deb.name}` : "cargo build --release",
      };

    case "macos":
      return {
        os,
        name: "macOS",
        primary: null,
        pending: "macOS build coming soon",
        alternatives: [],
        note: "No macOS build yet, and nobody has run DaCode there. It should compile; nothing is verified.",
        prompt: "#",
        command: "no macOS build yet",
      };

    default:
      return {
        os: "unknown",
        name: "your platform",
        primary: null,
        pending: null,
        alternatives: present([
          exe && { label: "Windows — dacode-setup.exe", detail: exe.name, url: exe.url, size: exe.size },
          deb && { label: "Linux — .deb", detail: deb.name, url: deb.url, size: deb.size },
          rpm && { label: "Linux — .rpm", detail: rpm.name, url: rpm.url, size: rpm.size },
          tar && { label: "Linux — .tar.gz", detail: tar.name, url: tar.url, size: tar.size },
        ]),
        note: "A desktop editor for x86-64 Linux and Windows. Pick your machine's build.",
        prompt: "#",
        command: "pick the artifact for your machine",
      };
  }
}
