
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
   * What to say in place of the button when there is nothing to download: a
   * label rather than a link, since there is no build to send anyone to.
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
 * Downloads come straight from the release on Gawad-B/DaCode, now that the
 * repository is public. While it was private its releases were private with
 * it: the asset URLs answered 404 to anyone not signed in as the owner, so every
 * button rendered with the right version, date and size and failed on click for
 * everyone else. That is why a separate, empty Gawad-B/dacode-releases
 * repository held copies of the assets for a while, and Google Drive before it.
 *
 * Nothing is fetched: a new release means writing its version and sizes in
 * here. `conventions.rs` fails the build if this table names a version the
 * crate is not.
 */
const ASSETS = "https://github.com/Gawad-B/DaCode/releases/download";

const RELEASE: Release = {
  version: "1.2.1",
  published: "2026-10-31",
  assets: {
    "dacode-setup.exe": {
      url: `${ASSETS}/v1.2.1/dacode-setup.exe`,
      size: 11067692,
    },
    "dacode-1.2.1-macos.dmg": {
      url: `${ASSETS}/v1.2.1/dacode-1.2.1-macos.dmg`,
      size: 25302867,
    },
    "dacode_1.2.1_amd64.deb": {
      url: `${ASSETS}/v1.2.1/dacode_1.2.1_amd64.deb`,
      size: 14224298,
    },
    "dacode-1.2.1-1.x86_64.rpm": {
      url: `${ASSETS}/v1.2.1/dacode-1.2.1-1.x86_64.rpm`,
      size: 14095409,
    },
    "dacode-1.2.1-x86_64-linux.tar.gz": {
      url: `${ASSETS}/v1.2.1/dacode-1.2.1-x86_64-linux.tar.gz`,
      size: 14225630,
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
  const dmg = find(release, (n) => n.endsWith(".dmg"));

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
        primary: dmg && {
          label: "Download for macOS",
          detail: dmg.name,
          url: dmg.url,
          size: dmg.size,
        },
        pending: dmg ? null : "macOS build coming soon",
        alternatives: [],
        // Both caveats, because both are true: an unsigned app the first launch must be talked
        // past, and a build no person has yet run on a Mac.
        note: "Apple Silicon and Intel, macOS 11+. Not Apple-signed: first open is right-click → Open. New, and not yet run on a real Mac.",
        prompt: "$",
        command: dmg ? `open ${dmg.name}` : "cargo build --release",
      };

    default:
      return {
        os: "unknown",
        name: "your platform",
        primary: null,
        pending: null,
        alternatives: present([
          exe && { label: "Windows — dacode-setup.exe", detail: exe.name, url: exe.url, size: exe.size },
          dmg && { label: "macOS — .dmg", detail: dmg.name, url: dmg.url, size: dmg.size },
          deb && { label: "Linux — .deb", detail: deb.name, url: deb.url, size: deb.size },
          rpm && { label: "Linux — .rpm", detail: rpm.name, url: rpm.url, size: rpm.size },
          tar && { label: "Linux — .tar.gz", detail: tar.name, url: tar.url, size: tar.size },
        ]),
        note: "A desktop editor for Linux, Windows and macOS. Pick your machine's build.",
        prompt: "#",
        command: "pick the artifact for your machine",
      };
  }
}
