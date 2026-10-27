export interface Contributor {
  login: string;
  avatar_url: string;
  html_url: string;
  contributions: number;
  type?: string;
}

interface ContributorsResult {
  contributors: Contributor[];
  total: number;
  fromCache: boolean;
}

// Snapshot of the top contributors on 2026-10-26, shown when GitHub is unreachable.
const FALLBACK: Contributor[] = [
  {
    login: "aaronmondal",
    avatar_url: "https://github.com/aaronmondal.png?size=80",
    html_url: "https://github.com/aaronmondal",
    contributions: 288,
  },
  {
    login: "astaplegun",
    avatar_url: "https://github.com/astaplegun.png?size=80",
    html_url: "https://github.com/astaplegun",
    contributions: 245,
  },
  {
    login: "palfrey",
    avatar_url: "https://github.com/palfrey.png?size=80",
    html_url: "https://github.com/palfrey",
    contributions: 229,
  },
  {
    login: "MarcusSorealheis",
    avatar_url: "https://github.com/MarcusSorealheis.png?size=80",
    html_url: "https://github.com/MarcusSorealheis",
    contributions: 224,
  },
  {
    login: "amankrx",
    avatar_url: "https://github.com/amankrx.png?size=80",
    html_url: "https://github.com/amankrx",
    contributions: 161,
  },
  {
    login: "chrisstaite-menlo",
    avatar_url: "https://github.com/chrisstaite-menlo.png?size=80",
    html_url: "https://github.com/chrisstaite-menlo",
    contributions: 68,
  },
  {
    login: "SchahinRohani",
    avatar_url: "https://github.com/SchahinRohani.png?size=80",
    html_url: "https://github.com/SchahinRohani",
    contributions: 59,
  },
  {
    login: "blakehatch",
    avatar_url: "https://github.com/blakehatch.png?size=80",
    html_url: "https://github.com/blakehatch",
    contributions: 43,
  },
  {
    login: "adam-singer",
    avatar_url: "https://github.com/adam-singer.png?size=80",
    html_url: "https://github.com/adam-singer",
    contributions: 36,
  },
  {
    login: "zbirenbaum",
    avatar_url: "https://github.com/zbirenbaum.png?size=80",
    html_url: "https://github.com/zbirenbaum",
    contributions: 23,
  },
  {
    login: "jaroeichler",
    avatar_url: "https://github.com/jaroeichler.png?size=80",
    html_url: "https://github.com/jaroeichler",
    contributions: 23,
  },
  {
    login: "corcillo",
    avatar_url: "https://github.com/corcillo.png?size=80",
    html_url: "https://github.com/corcillo",
    contributions: 18,
  },
];

/** Accounts not shown on the community page. */
const HIDDEN_LOGINS = new Set(["allada"]);

/** Drops bots and hidden accounts from a GitHub contributors listing. */
export { FALLBACK as FALLBACK_CONTRIBUTORS };

export function visibleContributors(contributors: Contributor[]): Contributor[] {
  return contributors.filter((c) => c.type !== "Bot" && !HIDDEN_LOGINS.has(c.login));
}

export async function getContributors(
  repo = "TraceMachina/nativelink",
): Promise<ContributorsResult> {
  const url = `https://api.github.com/repos/${repo}/contributors?per_page=30`;
  try {
    const res = await fetch(url, {
      headers: {
        Accept: "application/vnd.github+json",
        ...(process.env.GITHUB_TOKEN
          ? { Authorization: `Bearer ${process.env.GITHUB_TOKEN}` }
          : {}),
      },
      next: { revalidate: 60 * 60 * 6 },
    });

    if (!res.ok) {
      return { contributors: FALLBACK, total: FALLBACK.length, fromCache: true };
    }

    const json = (await res.json()) as Contributor[];
    const filtered = visibleContributors(json);
    return {
      contributors: filtered,
      total: filtered.length,
      fromCache: false,
    };
  } catch {
    return { contributors: FALLBACK, total: FALLBACK.length, fromCache: true };
  }
}

export interface RepoStats {
  stars: number;
  forks: number;
  openIssues: number;
  fromCache: boolean;
}

const FALLBACK_STATS: RepoStats = {
  stars: 1624,
  forks: 254,
  openIssues: 174,
  fromCache: true,
};

export async function getRepoStats(repo = "TraceMachina/nativelink"): Promise<RepoStats> {
  try {
    const res = await fetch(`https://api.github.com/repos/${repo}`, {
      headers: {
        Accept: "application/vnd.github+json",
        ...(process.env.GITHUB_TOKEN
          ? { Authorization: `Bearer ${process.env.GITHUB_TOKEN}` }
          : {}),
      },
      next: { revalidate: 60 * 60 * 6 },
    });
    if (!res.ok) return FALLBACK_STATS;
    const json = (await res.json()) as {
      stargazers_count: number;
      forks_count: number;
      open_issues_count: number;
    };
    return {
      stars: json.stargazers_count,
      forks: json.forks_count,
      openIssues: json.open_issues_count,
      fromCache: false,
    };
  } catch {
    return FALLBACK_STATS;
  }
}
