import { describe, expect, it } from "vitest";
import { canActivateText, isAmazonMirror } from "../src/model/mirror";
import type { SnapshotFolder, SnapshotNotebook } from "../src/model/types";

const F_AMAZON = "01a0c450-6c10-7101-8203-040506070801";
const F_WORK = "01a0c450-6c10-7101-8203-040506070802";
const F_LOWER = "01a0c450-6c10-7101-8203-040506070803";
const NB_AMAZON = "01a0c450-6c11-7101-8203-040506070801";
const NB_WORK = "01a0c450-6c11-7101-8203-040506070802";
const NB_TOP = "01a0c450-6c11-7101-8203-040506070803";
const NB_LOWER = "01a0c450-6c11-7101-8203-040506070804";
const NB_ORPHAN = "01a0c450-6c11-7101-8203-040506070805";

const nb = (notebook_id: string, folder_id?: string): SnapshotNotebook => ({
  notebook_id,
  title: "N",
  pages: [],
  ...(folder_id ? { folder_id } : {}),
});
const folders: SnapshotFolder[] = [
  { folder_id: F_AMAZON, title: "Amazon" },
  { folder_id: F_WORK, title: "Work" },
  { folder_id: F_LOWER, title: "amazon" },
];
const snap = {
  folders,
  notebooks: [
    nb(NB_AMAZON, F_AMAZON),
    nb(NB_WORK, F_WORK),
    nb(NB_TOP),
    nb(NB_LOWER, F_LOWER),
    nb(NB_ORPHAN, "01a0c450-6c10-7101-8203-0405060708ff"),
  ],
};

describe("isAmazonMirror", () => {
  it("is true for a notebook in the Amazon folder", () => expect(isAmazonMirror(snap, NB_AMAZON)).toBe(true));
  it("is false for a notebook in another folder", () => expect(isAmazonMirror(snap, NB_WORK)).toBe(false));
  it("is false for a top-level notebook", () => expect(isAmazonMirror(snap, NB_TOP)).toBe(false));
  it("is case-sensitive on the folder title", () => expect(isAmazonMirror(snap, NB_LOWER)).toBe(false));
  it("is false when folder_id matches no folder", () => expect(isAmazonMirror(snap, NB_ORPHAN)).toBe(false));
  it("is false for an unknown notebook", () => expect(isAmazonMirror(snap, "nope")).toBe(false));
});

describe("canActivateText", () => {
  const ok = { mirrorReadOnly: false, drawing: false, replicaAvailable: true, conflict: false };
  it("allows a normal notebook", () => expect(canActivateText(ok)).toBe(true));
  it("refuses on a mirror", () => expect(canActivateText({ ...ok, mirrorReadOnly: true })).toBe(false));
  it("refuses mid-stroke, without a replica, or in conflict", () => {
    expect(canActivateText({ ...ok, drawing: true })).toBe(false);
    expect(canActivateText({ ...ok, replicaAvailable: false })).toBe(false);
    expect(canActivateText({ ...ok, conflict: true })).toBe(false);
  });
});
