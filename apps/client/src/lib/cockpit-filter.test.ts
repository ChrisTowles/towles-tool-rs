import { describe, expect, it } from "vitest";
import { cockpitRepos, filterByRepo } from "./cockpit-filter";

describe("cockpitRepos", () => {
  it("collects the distinct repos across PRs and issues, sorted", () => {
    const prs = [{ repo: "octo/gizmos" }, { repo: "octo/widgets" }];
    const issues = [{ repo: "octo/widgets" }, { repo: "acme/api" }];
    expect(cockpitRepos(prs, issues)).toEqual(["acme/api", "octo/gizmos", "octo/widgets"]);
  });

  it("is empty when nothing is collected", () => {
    expect(cockpitRepos([], [])).toEqual([]);
  });
});

describe("filterByRepo", () => {
  const prs = [
    { repo: "octo/widgets", number: 1 },
    { repo: "octo/gizmos", number: 2 },
    { repo: "octo/widgets", number: 3 },
  ];

  it("returns everything when the selection is null (the All chip)", () => {
    expect(filterByRepo(prs, null)).toHaveLength(3);
  });

  it("narrows to only items in the selected repo", () => {
    expect(filterByRepo(prs, "octo/widgets").map((p) => p.number)).toEqual([1, 3]);
    expect(filterByRepo(prs, "octo/gizmos").map((p) => p.number)).toEqual([2]);
  });

  it("returns an empty list when no item matches the selection", () => {
    expect(filterByRepo(prs, "acme/api")).toEqual([]);
  });

  it("keeps PR and issue counts consistent under the same selection", () => {
    const issues = [
      { repo: "octo/widgets", number: 10 },
      { repo: "octo/gizmos", number: 11 },
    ];
    const selected = "octo/widgets";
    expect(filterByRepo(prs, selected)).toHaveLength(2);
    expect(filterByRepo(issues, selected)).toHaveLength(1);
  });
});
