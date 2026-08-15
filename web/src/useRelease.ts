
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
 * The download host, not the `/file/d/<id>/view` sharing link, which opens
 * Drive's preview page and leaves the visitor to find a second button. This one
 * answers with the bytes and a Content-Disposition, so the browser saves the
 * file where it would save any other download and the page never navigates
 * away. `confirm=t` is what skips the "Google Drive can't scan this file for
 * viruses" interstitial, which appears above about 100 MB — none of these are
 * close, but a future artifact could be.
 */
const drive = (id: string) =>
  `https://drive.usercontent.google.com/download?id=${id}&export=download&confirm=t`;

/*
 * The artifacts are served from Drive, and the version is written here by hand,
 * because the repository is private: its releases went private with it, so both
 * the assets and the API that used to list them answer 404 to anyone not signed
 * in as the owner. The page fetched that API and fell back to hard-coded GitHub
 * URLs when it failed — so after the repository was made private every download
 * button still rendered perfectly, with the right version, date and file size,
 * and 404'd on click. There was no failing state to see.
 *
 * The cost of doing it this way is that a new release must be uploaded and then
 * written in here. Each id is Drive's file id from the sharing link, and each
 * file must be shared as *anyone with the link* — otherwise it 404s for
 * everyone but the owner, which is exactly the failure this replaced.
 */
const RELEASE: Release = {
  version: "0.4.1",
  published: "2026-08-15",
  assets: {
    "vex-setup.exe": {
      url: drive("1fuXNEc2mkEL-wgr8PTeKtSTzuzQ5CzCY"),
      size: 7139700,
    },
    "vex_0.4.1_amd64.deb": {
      url: drive("1ZW1WDHAzTySeO04sikqPOXccE4xkp8iE"),
      size: 9004948,
    },
    "vex-0.4.1-1.x86_64.rpm": {
      url: drive("1xGw9_YbyrE_o7FIAavsDkeJslDFpArJ_"),
      size: 8902848,
    },
    "vex-0.4.1-x86_64-linux.tar.gz": {
      url: drive("1pRq7L0H1BrNHpqiJKMUTMMoaixrQ7NhJ"),
      // 9.7 MB, not the 9.0 MB the GitHub release carried: the file on Drive is
      // a different build. Taken from the Content-Length the host answers with.
      size: 9727998,
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
        note: "Per-user, under %LOCALAPPDATA%\\Programs\\vex — no admin prompt. Adds a Start Menu entry, vex on your PATH, “Open with vex” on a folder, and an uninstaller.",
        prompt: ">",
        command: ".\\vex-setup.exe",
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
        note: "Debian, Ubuntu and Mint. The tarball installs into ~/.local and needs no root at all.",
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
        note: "There is no macOS build yet, and nobody has ever run vex on macOS. It should compile — the code is portable — but nothing about the window, the mouse or the folder chooser has been verified there.",
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
          exe && { label: "Windows — vex-setup.exe", detail: exe.name, url: exe.url, size: exe.size },
          deb && { label: "Linux — .deb", detail: deb.name, url: deb.url, size: deb.size },
          rpm && { label: "Linux — .rpm", detail: rpm.name, url: rpm.url, size: rpm.size },
          tar && { label: "Linux — .tar.gz", detail: tar.name, url: tar.url, size: tar.size },
        ]),
        note: "vex is a desktop editor for x86-64 Linux and Windows. Pick the artifact for the machine you will run it on.",
        prompt: "#",
        command: "pick the artifact for your machine",
      };
  }
}
