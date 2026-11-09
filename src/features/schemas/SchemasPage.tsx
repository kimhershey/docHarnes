import type { TFunction } from "i18next";
import { Copy, FilePlus2, MessageSquareText, Save, Star, Trash2, Upload } from "lucide-react";
import { memo, useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { useNavigate, useSearchParams } from "react-router";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { ConfirmDialog, type ConfirmRequest } from "@/components/ui/confirm-dialog";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Spinner } from "@/components/ui/spinner";
import { Switch } from "@/components/ui/switch";
import { describeError } from "@/lib/errors";
import { api } from "@/lib/ipc";
import { useDocTypes, useDocuments, useSchemaVersions, useSchemas } from "@/lib/queries";
import { queryClient, queryKeys } from "@/lib/query-client";
import { draftFromSchema, newFieldDef, type SchemaDraft, type SchemaRecord } from "@/lib/types";
import { cn } from "@/lib/utils";
import { useAttachmentsStore } from "@/stores/attachments";
import { useUiStore } from "@/stores/ui";

import { FormRegisterDialog } from "./FormRegisterDialog";
import { NlSchemaDialog } from "./NlSchemaDialog";
import { SchemaEditor } from "./SchemaEditor";

function emptyDraft(): SchemaDraft {
  return { id: null, name: "", name_en: null, doc_type: "", source: "user", description_ko: null, description_en: null, definition: { fields: [newFieldDef("field_1")], rules: [] }, form_document_id: null, note: null };
}

/** A schema the user just saved under a new type slug: offers to re-classify the documents typed "other". */
interface NewType {
  slug: string;
  name: string;
  name_en: string | null;
}

/** One entry of the schema list; module-level + memoized so editor keystrokes neither remount nor re-render the list. */
const SchemaRow = memo(function SchemaRow({ s, selected, onSelect, ko, t }: { s: SchemaRecord; selected: boolean; onSelect: (s: SchemaRecord) => void; ko: boolean; t: TFunction }) {
  return (
    <button
      type="button"
      onClick={() => onSelect(s)}
      className={cn("flex w-full flex-col gap-1 rounded-md border px-2 py-1.5 text-left text-sm hover:bg-bg-sunken", selected ? "border-accent bg-accent-soft" : "border-transparent")}
      aria-current={selected ? "true" : undefined}
      data-testid={`schema-row-${s.id}`}
      data-schema-doc-type={s.doc_type}
    >
      <span className="flex items-center gap-1">
        <span className="truncate font-medium">{ko ? s.name : (s.name_en ?? s.name)}</span>
        {s.is_default && (
          <Badge variant="ok" className="text-[10px]">
            {t("schemas.default")}
          </Badge>
        )}
        {!s.enabled && (
          <Badge variant="muted" className="text-[10px]">
            {t("schemas.list.disabled")}
          </Badge>
        )}
      </span>
      <span className="flex items-center gap-1 text-xs text-fg-subtle">
        <code>{s.doc_type}</code> · {t(`schemas.source.${s.source}`)} · {t("schemas.version", { version: s.version })} · {t("schemas.uses", { count: s.use_count })}
      </span>
    </button>
  );
});

/**
 * 추출 스키마 / 양식 screen (design §3.7.2): registered forms + sample forms,
 * the field grid, NL create/modify, form registration, type → schema mapping,
 * version history, lint and a sample test run.
 */
export function SchemasPage() {
  const { t, i18n } = useTranslation();
  const navigate = useNavigate();
  const toast = useUiStore((s) => s.toast);
  const [params, setParams] = useSearchParams();
  const { data: schemas, isLoading } = useSchemas();
  const { data: docTypes } = useDocTypes();
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [draft, setDraft] = useState<SchemaDraft | null>(null);
  const [dirty, setDirty] = useState(false);
  const [busy, setBusy] = useState(false);
  const [nlOpen, setNlOpen] = useState(false);
  const [nlBase, setNlBase] = useState<SchemaRecord | null>(null);
  const [formOpen, setFormOpen] = useState(false);
  const [formDoc, setFormDoc] = useState<string | null>(null);
  // A document dropped on the sidebar's 스키마 entry registers as a form.
  const pendingFormDoc = useAttachmentsStore((s) => s.pendingFormDoc);
  useEffect(() => {
    if (!pendingFormDoc) return;
    useAttachmentsStore.getState().setPending({ pendingFormDoc: null });
    setFormDoc(pendingFormDoc.id);
    setFormOpen(true);
  }, [pendingFormDoc]);
  const [confirm, setConfirm] = useState<ConfirmRequest | null>(null);
  const [testDoc, setTestDoc] = useState("");
  const [newType, setNewType] = useState<NewType | null>(null);
  const { data: versions } = useSchemaVersions(draft?.id ?? null);
  const { data: docs } = useDocuments({ limit: 300, sort: "imported_desc" });
  const { data: otherDocs } = useDocuments({ doc_type: "other", limit: 1000 }, !!newType);
  const ko = i18n.language.startsWith("ko");

  const selected = useMemo(() => schemas?.find((s) => s.id === selectedId) ?? null, [schemas, selectedId]);
  const user = useMemo(() => (schemas ?? []).filter((s) => s.source !== "builtin"), [schemas]);
  const builtin = useMemo(() => (schemas ?? []).filter((s) => s.source === "builtin"), [schemas]);

  // `?register=<doc id>` from the library's "양식으로 등록" opens the form dialog on that document.
  const registerDoc = params.get("register");
  useEffect(() => {
    if (registerDoc) {
      setFormDoc(registerDoc);
      setFormOpen(true);
      const next = new URLSearchParams(params);
      next.delete("register");
      setParams(next, { replace: true });
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [registerDoc]);

  /**
   * Every replacement of a dirty draft (another schema, a new / duplicated / generated
   * draft) is confirmed first. In-app route changes are not blocked: the app runs on a
   * declarative `HashRouter`, and react-router's `useBlocker` needs a data router.
   */
  const guardDirty = useCallback(
    (action: () => void) => {
      if (!dirty) {
        action();
        return;
      }
      setConfirm({
        title: t("schemas.editor.discard"),
        description: t("schemas.editor.discard_confirm"),
        confirmLabel: t("schemas.editor.discard"),
        danger: true,
        onConfirm: action,
      });
    },
    [dirty, t],
  );
  const select = useCallback(
    (s: SchemaRecord) =>
      guardDirty(() => {
        setSelectedId(s.id);
        setDraft(draftFromSchema(s));
        setDirty(false);
      }),
    [guardDirty],
  );
  const openDraft = (d: SchemaDraft) => {
    setSelectedId(d.id);
    setDraft(d);
    setDirty(true);
  };
  const change = (d: SchemaDraft) => {
    setDraft(d);
    setDirty(true);
  };
  const newSchema = () => guardDirty(() => openDraft(emptyDraft()));
  const openNl = (base: SchemaRecord | null) =>
    guardDirty(() => {
      setNlBase(base);
      setNlOpen(true);
    });
  const openForm = () => guardDirty(() => setFormOpen(true));

  const save = async () => {
    if (!draft) return;
    setBusy(true);
    try {
      const saved = draft.source === "form" && !draft.id ? await api.pipelineFormRegister(draft) : await api.pipelineSchemaSave(draft);
      toast(draft.source === "form" && !draft.id ? t("schemas.editor.registered") : t("schemas.editor.saved"), "ok");
      if (!draft.id && !docTypes?.some((d) => d.slug === saved.doc_type)) setNewType({ slug: saved.doc_type, name: saved.name, name_en: saved.name_en });
      await queryClient.invalidateQueries({ queryKey: queryKeys.schemas });
      await queryClient.invalidateQueries({ queryKey: queryKeys.docTypes });
      setSelectedId(saved.id);
      setDraft(draftFromSchema(saved));
      setDirty(false);
    } catch (e) {
      toast(describeError(e, t), "danger");
    } finally {
      setBusy(false);
    }
  };
  const remove = (s: SchemaRecord) =>
    setConfirm({
      title: t("schemas.editor.delete"),
      description: t("schemas.editor.delete_confirm", { name: s.name }),
      confirmLabel: t("schemas.editor.delete"),
      danger: true,
      onConfirm: async () => {
        try {
          await api.pipelineSchemaDelete(s.id);
          toast(t("schemas.editor.deleted"), "ok");
          if (selectedId === s.id) {
            setSelectedId(null);
            setDraft(null);
            setDirty(false);
          }
        } catch (e) {
          toast(describeError(e, t), "danger");
        }
      },
    });
  const setDefault = async (s: SchemaRecord) => {
    try {
      await api.pipelineSchemaSetDefault(s.id);
    } catch (e) {
      toast(describeError(e, t), "danger");
    }
  };
  const setEnabled = async (s: SchemaRecord, enabled: boolean) => {
    try {
      await api.pipelineSchemaSetEnabled(s.id, enabled);
    } catch (e) {
      toast(describeError(e, t), "danger");
    }
  };
  // The Korean name gets the Korean suffix and the English name the English one, whatever the UI language.
  const duplicate = (s: SchemaRecord) =>
    guardDirty(() =>
      openDraft({
        ...draftFromSchema(s),
        id: null,
        name: i18n.getFixedT("ko")("schemas.editor.duplicate_name", { name: s.name }),
        name_en: s.name_en ? i18n.getFixedT("en")("schemas.editor.duplicate_name", { name: s.name_en }) : null,
        source: "user",
        form_document_id: null,
      }),
    );
  const testRun = async () => {
    if (!draft?.id || !testDoc) return;
    try {
      await api.pipelineProcess({ document_ids: [testDoc], level: "extract", schema: { kind: "id", id: draft.id }, force: true });
      toast(t("schemas.editor.test_started"), "info");
    } catch (e) {
      toast(describeError(e, t), "danger");
    }
  };
  const reclassify = async () => {
    const ids = (otherDocs ?? []).map((d) => d.id);
    if (ids.length === 0) return;
    try {
      await api.pipelineProcess({ document_ids: ids, level: "classify", force: true });
      toast(t("schemas.reclassify.started"), "info");
      setNewType(null);
    } catch (e) {
      toast(describeError(e, t), "danger");
    }
  };

  return (
    <div className="flex h-full min-h-0 flex-col" data-testid="page-schemas">
      <header className="flex flex-wrap items-center gap-2 border-b border-border px-4 py-3">
        <div className="mr-auto">
          <h1 className="text-lg font-semibold">{t("schemas.title")}</h1>
          <p className="text-xs text-fg-muted">{t("schemas.subtitle")}</p>
        </div>
        <Button size="sm" onClick={openForm} data-testid="schemas-register-form">
          <Upload />
          {t("schemas.register_form")}
        </Button>
        <Button size="sm" variant="secondary" onClick={() => openNl(null)} data-testid="schemas-nl-create">
          <MessageSquareText />
          {t("schemas.nl_create")}
        </Button>
        <Button size="sm" variant="secondary" onClick={newSchema} data-testid="schemas-new">
          <FilePlus2 />
          {t("schemas.new")}
        </Button>
      </header>
      {newType && (
        <div className="flex items-center gap-2 border-b border-border bg-info-soft px-4 py-1.5 text-xs" role="status" data-testid="schemas-reclassify-banner" data-new-type={newType.slug}>
          <span>{t("schemas.reclassify.banner", { name: ko ? newType.name : (newType.name_en ?? newType.name), count: otherDocs?.length ?? 0 })}</span>
          <Button size="sm" variant="secondary" className="h-6" onClick={() => void reclassify()} disabled={!otherDocs || otherDocs.length === 0}>
            {t("schemas.reclassify.run")}
          </Button>
          <Button size="sm" variant="ghost" className="h-6" onClick={() => setNewType(null)}>
            {t("common.close")}
          </Button>
        </div>
      )}
      <div className="flex min-h-0 flex-1">
        <aside className="flex w-72 shrink-0 flex-col gap-3 overflow-y-auto border-r border-border p-3" data-testid="schemas-list">
          {isLoading && <Spinner />}
          <div>
            <h2 className="mb-1 text-xs font-semibold uppercase tracking-wide text-fg-subtle">{t("schemas.list.user")}</h2>
            {user.length === 0 ? <p className="text-xs text-fg-muted">{t("schemas.list.empty")}</p> : user.map((s) => <SchemaRow key={s.id} s={s} selected={selectedId === s.id} onSelect={select} ko={ko} t={t} />)}
          </div>
          <div>
            <h2 className="mb-1 text-xs font-semibold uppercase tracking-wide text-fg-subtle">{t("schemas.list.builtin")}</h2>
            {builtin.map((s) => (
              <SchemaRow key={s.id} s={s} selected={selectedId === s.id} onSelect={select} ko={ko} t={t} />
            ))}
          </div>
          <Card>
            <CardHeader className="p-3">
              <CardTitle className="text-sm">{t("schemas.mapping.title")}</CardTitle>
              <CardDescription className="text-xs">{t("schemas.mapping.hint")}</CardDescription>
            </CardHeader>
            <CardContent className="p-3 pt-0">
              <table className="w-full text-xs" data-testid="schemas-mapping">
                <thead>
                  <tr className="text-left text-fg-subtle">
                    <th scope="col" className="py-1 font-medium">
                      {t("schemas.mapping.type")}
                    </th>
                    <th scope="col" colSpan={2} className="py-1 font-medium">
                      {t("schemas.mapping.schema")}
                    </th>
                  </tr>
                </thead>
                <tbody>
                  {(docTypes ?? []).map((d) => (
                    <tr key={d.slug} className="border-t border-border">
                      <td className="py-1 font-mono">{d.slug}</td>
                      <td className="py-1">{ko ? d.label_ko : d.label_en}</td>
                      <td className="py-1 text-right text-fg-subtle">{d.builtin ? t("schemas.mapping.builtin") : t("schemas.mapping.user")}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </CardContent>
          </Card>
        </aside>
        <section className="flex min-w-0 flex-1 flex-col gap-3 overflow-y-auto p-4">
          {!draft ? (
            <p className="text-sm text-fg-muted">{t("schemas.list.empty")}</p>
          ) : (
            <>
              <div className="flex flex-wrap items-center gap-2">
                <h2 className="text-base font-semibold" data-testid="schema-title">
                  {draft.id ? (selected ? (ko ? selected.name : (selected.name_en ?? selected.name)) : draft.name) : t("schemas.editor.new_draft")}
                </h2>
                {selected && <Badge variant="outline">{t(`schemas.source.${selected.source}`)}</Badge>}
                {dirty && <Badge variant="warn">{t("schemas.editor.unsaved")}</Badge>}
                <span className="ml-auto flex flex-wrap items-center gap-1">
                  {selected && (
                    <>
                      <label className="mr-2 flex items-center gap-1 text-xs">
                        <Switch checked={selected.enabled} onCheckedChange={(v) => void setEnabled(selected, v)} aria-label={t("schemas.enabled")} />
                        {t("schemas.enabled")}
                      </label>
                      <Button size="sm" variant="ghost" onClick={() => void setDefault(selected)} disabled={selected.is_default} data-testid="schema-set-default">
                        <Star />
                        {t("schemas.set_default")}
                      </Button>
                      <Button size="sm" variant="ghost" onClick={() => duplicate(selected)} data-testid="schema-duplicate">
                        <Copy />
                        {t("schemas.editor.duplicate")}
                      </Button>
                      <Button size="sm" variant="ghost" onClick={() => openNl(selected)} data-testid="schema-nl-modify">
                        <MessageSquareText />
                        {t("schemas.nl_modify")}
                      </Button>
                      {selected.source !== "builtin" && (
                        <Button size="sm" variant="ghost" className="text-danger" onClick={() => remove(selected)} data-testid="schema-delete">
                          <Trash2 />
                          {t("schemas.editor.delete")}
                        </Button>
                      )}
                      {selected.form_document_id && (
                        <Button size="sm" variant="ghost" onClick={() => navigate(`/library/${selected.form_document_id}`)}>
                          {t("schemas.editor.open_form_doc")}
                        </Button>
                      )}
                    </>
                  )}
                  <Button size="sm" onClick={() => void save()} disabled={busy} data-testid="schema-save">
                    <Save />
                    {draft.source === "form" && !draft.id ? t("schemas.form.register") : t("schemas.editor.save")}
                  </Button>
                </span>
              </div>
              {draft.source === "form" && !draft.id && <p className="rounded-md bg-info-soft px-3 py-1.5 text-xs">{t("schemas.form.confirm")}</p>}
              <SchemaEditor draft={draft} onChange={change} />
              {draft.id && (
                <div className="grid gap-3 md:grid-cols-2">
                  <Card>
                    <CardHeader className="p-3">
                      <CardTitle className="text-sm">{t("schemas.editor.versions")}</CardTitle>
                    </CardHeader>
                    <CardContent className="p-3 pt-0 text-xs">
                      <Input placeholder={t("schemas.editor.note")} value={draft.note ?? ""} onChange={(e) => change({ ...draft, note: e.target.value || null })} className="mb-2 h-7 text-xs" />
                      {!versions || versions.length === 0 ? (
                        <p className="text-fg-muted">{t("schemas.editor.no_versions")}</p>
                      ) : (
                        <ul className="flex flex-col gap-1" data-testid="schema-versions">
                          {versions.map((v) => (
                            <li key={v.version} className="flex items-center gap-2">
                              <Badge variant="outline">{t("schemas.version", { version: v.version })}</Badge>
                              <span className="truncate text-fg-muted">{v.note ?? ""}</span>
                              <Button size="sm" variant="ghost" className="ml-auto h-6" onClick={() => change({ ...draft, definition: structuredClone(v.definition) })}>
                                {t("schemas.editor.restore_version")}
                              </Button>
                            </li>
                          ))}
                        </ul>
                      )}
                    </CardContent>
                  </Card>
                  <Card>
                    <CardHeader className="p-3">
                      <CardTitle className="text-sm">{t("schemas.editor.test")}</CardTitle>
                      <CardDescription className="text-xs">{t("schemas.editor.test_hint")}</CardDescription>
                    </CardHeader>
                    <CardContent className="flex items-center gap-2 p-3 pt-0">
                      <Select value={testDoc} onValueChange={setTestDoc}>
                        <SelectTrigger className="h-8 flex-1" aria-label={t("schemas.editor.test_pick")} data-testid="schema-test-doc">
                          <SelectValue placeholder={t("schemas.editor.test_pick")} />
                        </SelectTrigger>
                        <SelectContent>
                          {(docs ?? []).map((d) => (
                            <SelectItem key={d.id} value={d.id}>
                              {d.title ?? d.original_name}
                            </SelectItem>
                          ))}
                        </SelectContent>
                      </Select>
                      <Button size="sm" variant="secondary" onClick={() => void testRun()} disabled={!testDoc} data-testid="schema-test-run">
                        {t("schemas.editor.test")}
                      </Button>
                    </CardContent>
                  </Card>
                </div>
              )}
            </>
          )}
        </section>
      </div>
      <NlSchemaDialog open={nlOpen} onOpenChange={setNlOpen} base={nlBase} onDraft={openDraft} />
      <FormRegisterDialog open={formOpen} onOpenChange={(o) => { setFormOpen(o); if (!o) setFormDoc(null); }} documentId={formDoc} onDraft={openDraft} />
      <ConfirmDialog request={confirm} onClose={() => setConfirm(null)} />
    </div>
  );
}
