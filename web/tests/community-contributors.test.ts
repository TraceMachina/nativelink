import { describe, expect, test } from "bun:test";
import { type Contributor, visibleContributors } from "../apps/web/lib/github";

function contributor(login: string, type?: string): Contributor {
  return {
    login,
    avatar_url: `https://github.com/${login}.png?size=80`,
    html_url: `https://github.com/${login}`,
    contributions: 1,
    type,
  };
}

describe("visibleContributors", () => {
  test("drops bots and hidden accounts, keeps everyone else in order", () => {
    const listing = [
      contributor("aaronmondal", "User"),
      contributor("renovate[bot]", "Bot"),
      contributor("allada", "User"),
      contributor("blaise-d", "User"),
    ];
    expect(visibleContributors(listing).map((c) => c.login)).toEqual(["aaronmondal", "blaise-d"]);
  });
});
