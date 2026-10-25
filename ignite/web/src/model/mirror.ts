import type { SyncSnapshotV1 } from "./types";

/** The Kindle mirrors Amazon's stock notebooks into a folder with exactly this title. */
export const AMAZON_FOLDER_TITLE = "Amazon";

/** True when the notebook lives in a folder titled exactly `Amazon` (read-only by convention). */
export function isAmazonMirror(snapshot: Pick<SyncSnapshotV1, "notebooks" | "folders">, notebookId: string): boolean {
  const folderId = snapshot.notebooks.find((n) => n.notebook_id === notebookId)?.folder_id;
  if (!folderId) return false;
  return snapshot.folders.some((f) => f.folder_id === folderId && f.title === AMAZON_FOLDER_TITLE);
}
