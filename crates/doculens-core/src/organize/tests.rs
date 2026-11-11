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
        db.with(pipeline_repo::enable_legacy_builtins).unwrap();
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

#[test]
fn project_variables_and_project_scope() {
    use crate::projects::{copies, repo as projects_repo, ProjectInput, ProjectSettings, DEFAULT_PROJECT_FOLDER};
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
    let p =
        w.db.with(|c| {
            projects_repo::save_project(
                c,
                &ProjectInput {
                    name: "한빛 포털 구축".into(),
                    intake_no: Some("2026-001".into()),
                    owner: Some("박서연".into()),
                    start_date: Some("2026-01-01".into()),
                    end_date: Some("2099-12-31".into()),
                    settings: Some(ProjectSettings {
                        folder_template: Some("archive/{{project}}/{{year}}".into()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            )
        })
        .unwrap();
    copies::add_documents(&w.db, &w.paths, &p.id, &["a".to_string()], Language::Ko).unwrap();
    assert_eq!(folder_template(GroupBy::Project), DEFAULT_PROJECT_FOLDER);

    // Every `{{project*}}` variable renders from the document's project; a document without one is 미상.
    let req = OrganizeRequest {
        scope: Scope { folder: Some("inbox".into()), ..Scope::default() },
        name_template: Some(
            "{{project_code}}_{{project_phase}}_{{project_start}}_{{project_end}}_{{project_owner}}_{{title}}".into(),
        ),
        folder_template: Some(folder_template(GroupBy::Project).into()),
        only_reviewed: false,
        language: Language::Ko,
    };
    let plan1 = plan(&w.db, &w.paths, &req).unwrap();
    let by_id = |p: &OrganizePlan, id: &str| p.items.iter().find(|i| i.doc_id == id).unwrap().clone();
    assert_eq!(
        by_id(&plan1, "a").to.as_deref(),
        Some("documents/한빛 포털 구축/영수증/2026-001_진행중_2026-01-01_2099-12-31_박서연_IMG_2031.jpg")
    );
    let b = by_id(&plan1, "b");
    assert_eq!(b.to.as_deref(), Some("documents/미상/영수증/미상_미상_미상_미상_미상_scan_0007.pdf"));
    assert!(b.missing.contains(&"project".to_string()) && b.missing.contains(&"project_owner".to_string()), "{b:?}");
    assert_eq!(plan1.needs_review, 1);

    // A project scope narrows the documents to the members; the English phase label follows the language.
    let scoped = OrganizeRequest {
        scope: Scope {
            filter: ScopeFilter { project_id: Some(p.id.clone()), ..Default::default() },
            ..Scope::default()
        },
        name_template: Some("{{project_phase}}_{{title}}".into()),
        folder_template: Some("{{project}}".into()),
        only_reviewed: false,
        language: Language::En,
    };
    let plan2 = plan(&w.db, &w.paths, &scoped).unwrap();
    assert_eq!(plan2.items.iter().map(|i| i.doc_id.as_str()).collect::<Vec<_>>(), vec!["a"]);
    assert_eq!(by_id(&plan2, "a").to.as_deref(), Some("한빛 포털 구축/Active_IMG_2031.jpg"));

    // Applying a move keeps the project's copy of a shared document in sync (it follows the rename).
    let other =
        w.db.with(|c| projects_repo::save_project(c, &ProjectInput { name: "두번째".into(), ..Default::default() }))
            .unwrap();
    copies::add_documents(&w.db, &w.paths, &other.id, &["a".to_string()], Language::Ko).unwrap();
    let before = w.db.with(|c| copies::list_for_project(c, &other.id)).unwrap().remove(0);
    assert_eq!(before.rel_path, "documents/두번째/영수증/IMG_2031.jpg");
    let applied = apply(&w.db, &w.paths, &plan2, "organize", "프로젝트 정리", "test", None, &mut no_progress).unwrap();
    assert_eq!((applied.moved, applied.failed.len()), (1, 0), "{applied:?}");
    let after = w.db.with(|c| copies::list_for_project(c, &other.id)).unwrap().remove(0);
    assert_eq!(after.rel_path, "documents/두번째/영수증/Active_IMG_2031.jpg", "{after:?}");
    assert!(w.paths.root.join(&after.rel_path).is_file() && !w.paths.root.join(&before.rel_path).is_file());
    assert!(w.paths.root.join("한빛 포털 구축/Active_IMG_2031.jpg").is_file());
}

/// PRJ-7: "이 프로젝트 정리" files the project's own documents. A document that is in the project through a
/// tracked copy (its original belongs to an earlier project) is left where it is — the original used to be
/// moved next to its own copy (`_2`, then `_2_2` for the copy) and out of its first project's folder.
#[test]
fn project_scope_organise_leaves_documents_filed_there_as_copies() {
    use crate::projects::{copies, repo as projects_repo, ProjectInput, DEFAULT_PROJECT_FOLDER};
    let w = World::new();
    w.doc("a", "documents/A 사업/계약서/계약서.pdf", b"aaa", Some("contract"), None, None, None, ReviewStatus::Done);
    w.doc("b", "inbox/검수확인서.pdf", b"bbb", Some("contract"), None, None, None, ReviewStatus::Done);
    let project = |name: &str| {
        w.db.with(|c| projects_repo::save_project(c, &ProjectInput { name: name.into(), ..Default::default() }))
            .unwrap()
    };
    let (first, second) = (project("A 사업"), project("B 사업"));
    copies::add_documents(&w.db, &w.paths, &first.id, &["a".to_string()], Language::Ko).unwrap();
    copies::add_documents(&w.db, &w.paths, &second.id, &["a".to_string(), "b".to_string()], Language::Ko).unwrap();
    let copy = w.db.with(|c| copies::list_for_project(c, &second.id)).unwrap().remove(0);
    assert_eq!((copy.document_id.as_str(), copy.rel_path.as_str()), ("a", "documents/B 사업/계약서/계약서.pdf"));

    let req = OrganizeRequest {
        scope: Scope {
            filter: ScopeFilter { project_id: Some(second.id.clone()), ..Default::default() },
            ..Scope::default()
        },
        name_template: None,
        folder_template: Some(DEFAULT_PROJECT_FOLDER.into()),
        only_reviewed: false,
        language: Language::Ko,
    };
    let p = plan(&w.db, &w.paths, &req).unwrap();
    let by_id = |id: &str| p.items.iter().find(|i| i.doc_id == id).unwrap().clone();
    // The shared document: nothing to do here, its copy already is this project's file.
    let shared = by_id("a");
    assert_eq!(
        (shared.status, shared.to.as_deref(), shared.reason.as_deref()),
        (ItemStatus::Unchanged, None, Some("project_copy"))
    );
    // The project's own document is filed as usual.
    assert_eq!(by_id("b").to.as_deref(), Some("documents/B 사업/계약서/검수확인서.pdf"));
    assert_eq!((p.moves, p.unchanged), (1, 1));
    let applied = apply(&w.db, &w.paths, &p, "organize", "프로젝트 정리", "test", None, &mut no_progress).unwrap();
    assert_eq!((applied.moved, applied.failed.len()), (1, 0), "{applied:?}");
    // The original stays in its first project's folder and the copy keeps its name.
    assert_eq!(w.rel_path("a"), "documents/A 사업/계약서/계약서.pdf");
    assert!(w.paths.root.join("documents/A 사업/계약서/계약서.pdf").is_file());
    let copies_now = w.db.with(|c| copies::list_for_project(c, &second.id)).unwrap();
    assert_eq!(copies_now.len(), 1);
    assert_eq!(copies_now[0].rel_path, "documents/B 사업/계약서/계약서.pdf");
    let mut files: Vec<String> = std::fs::read_dir(w.paths.root.join("documents/B 사업/계약서"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    files.sort();
    assert_eq!(files, vec!["검수확인서.pdf", "계약서.pdf"], "no `_2` duplicates");
    // Organising the first project still moves the original, and the copy follows its new name.
    let home = OrganizeRequest {
        scope: Scope {
            filter: ScopeFilter { project_id: Some(first.id.clone()), ..Default::default() },
            ..Scope::default()
        },
        name_template: Some("{{doc_type}}_{{title}}".into()),
        ..req.clone()
    };
    let p = plan(&w.db, &w.paths, &home).unwrap();
    assert_eq!(
        p.items.iter().find(|i| i.doc_id == "a").unwrap().to.as_deref(),
        Some("documents/A 사업/계약서/계약서_계약서.pdf")
    );
    apply(&w.db, &w.paths, &p, "organize", "프로젝트 정리", "test", None, &mut no_progress).unwrap();
    let followed = w.db.with(|c| copies::list_for_project(c, &second.id)).unwrap().remove(0);
    assert_eq!(followed.rel_path, "documents/B 사업/계약서/계약서_계약서.pdf");
}
