import { PutInProjectDialog } from "@/features/projects/PutInProjectDialog";

/**
 * @deprecated 0.5.0: "지식 베이스에 추가" became "프로젝트에 넣기" with the common reference material preselected.
 * Kept as a thin wrapper for the library / viewer entry points until they switch to `PutInProjectDialog`.
 */
export function AddToCollectionDialog({ open, onOpenChange, docIds, onDone }: { open: boolean; onOpenChange: (open: boolean) => void; docIds: string[]; onDone?: () => void }) {
  return <PutInProjectDialog open={open} onOpenChange={onOpenChange} docIds={docIds} defaultTarget="common" onDone={() => onDone?.()} />;
}
