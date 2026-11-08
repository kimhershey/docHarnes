use std::sync::atomic::AtomicBool;

use chrono::Utc;

use crate::db::Db;
use crate::documents::convert::StubPdfPrinter;
use crate::documents::import::ImportDeps;
use crate::documents::render::RenderOptions;
use crate::documents::{repo as docs, DocumentRole, DocumentStatus, NewDocument, ReviewStatus, SourceKind};
use crate::paths::WorkspacePaths;
use crate::pipeline::repo as pipeline_repo;
use crate::planner::{Scope, ScopeFilter};

use super::journal::{self, EntryKind};
use super::*;

struct World {
    db: Db,
    paths: WorkspacePaths,
    _dir: tempfile::TempDir,
}

impl World {
    fn new() -> World {
        let dir = tempfile::tempdir().unwrap();
        let paths = WorkspacePaths::new(dir.path());
        for d in paths.all_dirs() {
            std::fs::create_dir_all(d).unwrap();
        }
        let db = Db::open_in_memory().unwrap();
        db.with(pipeline_repo::ensure_builtins).unwrap();
        World { db, paths, _dir: dir }
    }

    fn deps(&self) -> ImportDeps<'_> {
        ImportDeps {
            db: &self.db,
            paths: &self.paths,
            render: None,
            printer: &StubPdfPrinter,
            render_options: RenderOptions::default(),
        }
    }

    /// Registers a file at `rel` with the given bytes and extraction metadata.
    #[allow(clippy::too_many_arguments)]
    fn doc(
        &self,
        id: &str,
        rel: &str,
        bytes: &[u8],
        doc_type: Option<&str>,
        vendor: Option<&str>,
        date: Option<&str>,
        total: Option<f64>,
        review: ReviewStatus,
    ) {
        let abs = self.paths.root.join(rel);
        std::fs::create_dir_all(abs.parent().unwrap()).unwrap();
        std::fs::write(&abs, bytes).unwrap();
        let hash = crate::documents::import::blake3_file(&abs).unwrap();
        let name = rel.rsplit('/').next().unwrap().to_string();
        self.db
            .with(|c| {
                docs::insert_document(
                    c,
                    &NewDocument {
                        id: id.into(),
                        rel_path: rel.into(),
                        original_name: name,
                        source_kind: SourceKind::Pdf,
                        content_hash: hash,
                        size_bytes: bytes.len() as u64,
                        file_modified_at: Some(Utc::now()),
                        role: DocumentRole::Work,
                        import_batch_id: None,
                    },
                )?;
                docs::set_status(c, id, DocumentStatus::Rendered, None)?;
                if let Some(t) = doc_type {
                    docs::set_doc_type(c, id, Some(t), Some("auto"))?;
                }
                c.execute(
                    "UPDATE documents SET vendor = ?2, doc_date = ?3, total_amount = ?4, review_status = ?5 WHERE id = ?1",
                    rusqlite::params![id, vendor, date, total, review.as_str()],
                )?;
                Ok(())
            })
            .unwrap();
    }

    fn rel_path(&self, id: &str) -> String {
        self.db.with(|c| docs::get_document(c, id)).unwrap().unwrap().rel_path
    }
}

fn no_progress(_: u64, _: u64, _: &str) {}

#[test]
fn plan_renders_templates_holds_pending_and_resolves_collisions() {
    let w = World::new();
    w.doc(
        "a",
        "inbox/IMG_2031.jpg",
        b"aaa",
        Some("receipt"),
        Some("스타벅스"),
        Some("2026-10-14"),
        Some(12500.0),
        ReviewStatus::Done,
    );
    w.doc(
        "b",
        "inbox/scan_0007.pdf",
        b"bbb",
        Some("receipt"),
        Some("GS25"),
        Some("2026-10-02"),
        Some(8900.0),
        ReviewStatus::None,
    );
    w.doc(
        "c",
        "inbox/scan_0008.pdf",
        b"ccc",
        Some("receipt"),
        None,
        Some("2026-10-02"),
        Some(8900.0),
        ReviewStatus::Pending,
    );
    // Same rendered name as `b` (same vendor / date / total) → `_2`.
    w.doc(
        "d",
        "inbox/scan_0009.pdf",
        b"ddd",
        Some("receipt"),
        Some("GS25"),
        Some("2026-10-02"),
        Some(8900.0),
        ReviewStatus::Done,
    );
    // No metadata at all → 미상 placeholders + needs_review.
    w.doc("e", "inbox/unknown.pdf", b"eee", None, None, None, None, ReviewStatus::None);
    let req = OrganizeRequest {
        scope: Scope { folder: Some("inbox".into()), ..Scope::default() },
        name_template: Some(NAME_TEMPLATE.into()),
        folder_template: Some(folder_template(GroupBy::Month).into()),
        only_reviewed: true,
        language: Language::Ko,
    };
    let p = plan(&w.db, &w.paths, &req).unwrap();
    let by_id = |id: &str| p.items.iter().find(|i| i.doc_id == id).unwrap();
    assert_eq!(by_id("a").to.as_deref(), Some("documents/2026/10/영수증/2026-10-14_영수증_스타벅스_12,500.jpg"));
    assert_eq!(by_id("b").to.as_deref(), Some("documents/2026/10/영수증/2026-10-02_영수증_GS25_8,900.pdf"));
    assert_eq!(by_id("d").to.as_deref(), Some("documents/2026/10/영수증/2026-10-02_영수증_GS25_8,900_2.pdf"));
    assert_eq!(by_id("c").status, ItemStatus::Held);
    assert_eq!(by_id("c").reason.as_deref(), Some("review_pending"));
    let e = by_id("e");
    assert!(e.to.as_deref().unwrap().contains("미상"), "{e:?}");
    assert!(e.to.as_deref().unwrap().starts_with(&format!("documents/{}/", Utc::now().format("%Y"))), "{e:?}");
    assert!(e.missing.contains(&"vendor".to_string()) && e.missing.contains(&"total".to_string()));
    assert_eq!((p.moves, p.held, p.unchanged, p.needs_review), (4, 1, 0, 1));

    // Unknown variable → error
    let bad = OrganizeRequest { name_template: Some("{{nope}}".into()), ..req.clone() };
    assert!(plan(&w.db, &w.paths, &bad).is_err());
}

#[test]
fn apply_moves_files_journals_and_undo_redo_round_trip() {
    let w = World::new();
    w.doc(
        "a",
        "inbox/IMG_2031.jpg",
        b"aaa",
        Some("receipt"),
        Some("스타벅스"),
        Some("2026-10-14"),
        Some(12500.0),
        ReviewStatus::Done,
    );
    w.doc(
        "b",
        "inbox/scan_0007.pdf",
        b"bbb",
        Some("receipt"),
        Some("GS25"),
        Some("2026-10-02"),
        Some(8900.0),
        ReviewStatus::Done,
    );
    let req = OrganizeRequest {
        scope: Scope {
            filter: ScopeFilter { doc_type: Some("receipt".into()), ..ScopeFilter::default() },
            ..Scope::default()
        },
        name_template: Some(NAME_TEMPLATE.into()),
        folder_template: Some(folder_template(GroupBy::Month).into()),
        only_reviewed: true,
        language: Language::Ko,
    };
    let p = plan(&w.db, &w.paths, &req).unwrap();
    let mut calls = 0;
    let r = apply(&w.db, &w.paths, &p, "organize", "월별 정리", "test", None, &mut |_, _, _| calls += 1).unwrap();
    assert_eq!(r.moved, 2);
    assert!(r.failed.is_empty());
    assert_eq!(calls, 2);
    let batch = r.batch_id.clone().unwrap();
    let a_new = "documents/2026/10/영수증/2026-10-14_영수증_스타벅스_12,500.jpg";
    assert!(w.paths.root.join(a_new).is_file());
    assert!(!w.paths.root.join("inbox/IMG_2031.jpg").exists());
    assert_eq!(w.rel_path("a"), a_new);
    let entries = w.db.with(|c| journal::entries(c, &batch)).unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].kind, EntryKind::Move);
    assert!(w.paths.journal_dir().join(format!("ops-{}.jsonl", Utc::now().format("%Y%m"))).is_file());
    let batches = w.db.with(|c| journal::list_batches(c, 10)).unwrap();
    assert_eq!(batches[0].id, batch);
    assert!(batches[0].undoable());

    // The plan is a dry run: applying it twice must not move anything again.
    let again = apply(&w.db, &w.paths, &p, "organize", "again", "test", None, &mut no_progress).unwrap();
    assert_eq!(again.moved, 0);
    assert_eq!(again.failed.len(), 2);
    assert!(again.batch_id.is_none());

    // Tamper with one moved file → that entry is skipped on undo, the other restored.
    std::fs::write(w.paths.root.join("documents/2026/10/영수증/2026-10-02_영수증_GS25_8,900.pdf"), b"changed").unwrap();
    let u = journal::undo_last(&w.db, &w.paths).unwrap().unwrap();
    assert_eq!(u.source_batch, batch);
    assert_eq!(u.restored, 1);
    assert_eq!(u.skipped.len(), 1);
    assert_eq!(u.skipped[0].1, "changed");
    assert_eq!(w.rel_path("a"), "inbox/IMG_2031.jpg");
    assert!(w.paths.root.join("inbox/IMG_2031.jpg").is_file());
    assert_eq!(w.rel_path("b"), "documents/2026/10/영수증/2026-10-02_영수증_GS25_8,900.pdf");
    let batches = w.db.with(|c| journal::list_batches(c, 10)).unwrap();
    assert_eq!(batches[0].kind, "undo");
    assert_eq!(batches[0].redo_of.as_deref(), Some(batch.as_str()));
    assert!(batches[1].undone_at.is_some());
    assert!(w.db.with(journal::last_undoable).unwrap().is_none());

    // Redo re-applies the undo (one step), then undo works again.
    let r2 = journal::redo_last(&w.db, &w.paths).unwrap().unwrap();
    assert_eq!(r2.restored, 1);
    assert_eq!(w.rel_path("a"), a_new);
    assert!(w.db.with(journal::last_redoable).unwrap().is_none());
    let u2 = journal::undo_last(&w.db, &w.paths).unwrap().unwrap();
    assert_eq!(u2.source_kind, "redo");
    assert_eq!(w.rel_path("a"), "inbox/IMG_2031.jpg");
    // Nothing left to redo/undo of the same kind beyond the chain.
    assert!(journal::undo_batch(&w.db, &w.paths, &batch).is_err());

    // Purge keeps recent batches.
    assert_eq!(w.db.with(|c| journal::purge(c, 90)).unwrap(), 0);
}

#[test]
fn deletion_to_workspace_trash_is_journaled_and_restorable() {
    let w = World::new();
    w.doc("a", "inbox/IMG_2031.jpg", b"aaa", Some("receipt"), None, None, None, ReviewStatus::None);
    let deps = w.deps();
    let cancel = AtomicBool::new(false);
    let r = delete_documents(&deps, &["a".into(), "zzz".into()], "삭제", "test", Some(&cancel)).unwrap();
    assert_eq!(r.moved, 1);
    assert_eq!(r.failed.len(), 1);
    let batch = r.batch_id.clone().unwrap();
    let entries = w.db.with(|c| journal::entries(c, &batch)).unwrap();
    assert_eq!(entries[0].kind, EntryKind::Delete);
    assert!(w.db.with(|c| docs::get_document(c, "a")).unwrap().is_none());
    let trashed_in_workspace = entries[0].trash_path.is_some();
    let u = journal::undo_batch(&w.db, &w.paths, &batch).unwrap();
    if trashed_in_workspace {
        assert_eq!(u.restored, 1);
        assert_eq!(u.restored_paths, vec!["inbox/IMG_2031.jpg".to_string()]);
        assert!(w.paths.root.join("inbox/IMG_2031.jpg").is_file());
    } else {
        // OS recycle bin: restoring is left to the OS.
        assert_eq!(u.skipped[0].1, "os_trash");
    }
}

#[test]
fn undo_with_nothing_restorable_keeps_the_batch_and_redo_is_invalidated_by_newer_batches() {
    let w = World::new();
    w.doc("a", "inbox/a.pdf", b"aaa", Some("receipt"), Some("A"), Some("2026-10-01"), Some(1.0), ReviewStatus::Done);
    let req = OrganizeRequest {
        scope: Scope { folder: Some("inbox".into()), ..Scope::default() },
        name_template: None,
        folder_template: Some("documents/{{doc_type}}".into()),
        only_reviewed: false,
        language: Language::Ko,
    };
    let p = plan(&w.db, &w.paths, &req).unwrap();
    let r = apply(&w.db, &w.paths, &p, "move", "이동", "test", None, &mut no_progress).unwrap();
    let batch = r.batch_id.clone().unwrap();
    // The moved file disappears (removable drive): nothing can be restored, the batch stays undoable.
    let moved = w.paths.root.join("documents/영수증/a.pdf");
    std::fs::remove_file(&moved).unwrap();
    let u = journal::undo_batch(&w.db, &w.paths, &batch).unwrap();
    assert_eq!((u.restored, u.skipped.len()), (0, 1));
    assert_eq!(u.skipped[0].1, "missing");
    assert!(u.batch_id.is_empty());
    assert!(w.db.with(|c| journal::get_batch(c, &batch)).unwrap().unwrap().undone_at.is_none());
    assert_eq!(w.db.with(|c| journal::list_batches(c, 10)).unwrap().len(), 1, "no empty undo batch left behind");
    // The file comes back → the same batch can be undone now.
    std::fs::write(&moved, b"aaa").unwrap();
    let u = journal::undo_batch(&w.db, &w.paths, &batch).unwrap();
    assert_eq!(u.restored, 1);
    assert_eq!(w.rel_path("a"), "inbox/a.pdf");
    assert!(w.db.with(journal::last_redoable).unwrap().is_some());
    // A newer file batch invalidates the redo stack.
    std::thread::sleep(std::time::Duration::from_millis(5));
    w.doc("b", "inbox/b.pdf", b"bbb", Some("receipt"), Some("B"), Some("2026-10-02"), Some(2.0), ReviewStatus::Done);
    let req_b = OrganizeRequest { scope: Scope { doc_ids: vec!["b".into()], ..Scope::default() }, ..req.clone() };
    let pb = plan(&w.db, &w.paths, &req_b).unwrap();
    apply(&w.db, &w.paths, &pb, "move", "이동 b", "test", None, &mut no_progress).unwrap();
    assert!(w.db.with(journal::last_redoable).unwrap().is_none());
}

#[test]
fn redo_never_re_deletes_a_restored_document() {
    let w = World::new();
    w.doc("a", "inbox/a.pdf", b"aaa", Some("receipt"), None, None, None, ReviewStatus::None);
    let deps = w.deps();
    let r = delete_documents(&deps, &["a".into()], "삭제", "test", None).unwrap();
    let batch = r.batch_id.clone().unwrap();
    let entries = w.db.with(|c| journal::entries(c, &batch)).unwrap();
    if entries[0].trash_path.is_none() {
        return; // OS recycle bin on this machine: nothing to restore in-app
    }
    let u = journal::undo_batch(&w.db, &w.paths, &batch).unwrap();
    assert_eq!(u.restored, 1);
    assert!(w.paths.root.join("inbox/a.pdf").is_file());
    // "Redo" of that restore would trash the file again under a live (re-imported) row: refused.
    let redo = journal::redo_last(&w.db, &w.paths).unwrap().unwrap();
    assert_eq!(redo.restored, 0);
    assert_eq!(redo.skipped[0].1, "restore_not_redoable");
    assert!(w.paths.root.join("inbox/a.pdf").is_file());
}
