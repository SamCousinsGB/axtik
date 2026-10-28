import * as vscode from 'vscode';
import * as path from 'node:path';
import * as fs from 'node:fs/promises';
import { runEngine, resolveBundledEngine } from '../../runner/src/index';
import { containedBy, needsScopedWatcher, normalizeTargets, RequestLedger, snapshotSelectedTests, summaryLine, testPresentation, type ValidationReport, type TestIdentity } from './model';

type Command = 'check' | 'validate' | 'test';
type Group = { root: string; paths: string[] };
const active = new Map<string, AbortController>();

export function activate(context: vscode.ExtensionContext): void {
  const output = vscode.window.createOutputChannel('APX Local Apex');
  const diagnostics = vscode.languages.createDiagnosticCollection('apx');
  const controller = vscode.tests.createTestController('apx.preview', 'APX Local Tests (Preview)');
  const ledger = new RequestLedger();
  const diagnosticGroups = new Map<string, Map<string, vscode.Diagnostic[]>>();
  const identities = new Map<string, TestIdentity>();
  const environmentFiles = new Map<string, string>();
  const environmentWatchers = new Map<string, vscode.Disposable[]>();
  const status = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Left, 35);
  status.text = '$(beaker) APX Preview';
  status.tooltip = 'Local syntax checks and experimental Apex tests. No Salesforce certification.';
  status.command = 'apx.showOutput';
  status.show();
  context.subscriptions.push(output, diagnostics, controller, status);

  const errorText = (e: unknown): string => e instanceof Error ? e.message : String(e);
  const register = (id: string, fn: (...args: any[]) => Promise<unknown> | unknown): void => {
    context.subscriptions.push(vscode.commands.registerCommand(id, async (...args) => {
      try { return await fn(...args); }
      catch (e) { output.appendLine(`ERROR: ${errorText(e)}`); void vscode.window.showErrorMessage(`APX: ${errorText(e)}`, 'Show Output').then(a => { if (a) output.show(); }); }
    }));
  };

  function renderDiagnostics(): void {
    const merged = new Map<string, vscode.Diagnostic[]>();
    for (const group of diagnosticGroups.values()) for (const [file, items] of group) {
      merged.set(file, [...(merged.get(file) ?? []), ...items]);
    }
    diagnostics.clear();
    for (const [file, items] of merged) diagnostics.set(vscode.Uri.file(file), items);
  }

  function applyReport(report: ValidationReport, root: string): void {
    const grouped = new Map<string, vscode.Diagnostic[]>();
    for (const item of report.diagnostics) {
      if (!item.path || !path.isAbsolute(item.path)) continue;
      const start = new vscode.Position(Math.max(0, item.line - 1), Math.max(0, item.column - 1));
      const proposedEnd = new vscode.Position(Math.max(0, item.endLine - 1), Math.max(0, item.endColumn - 1));
      const end = proposedEnd.isBefore(start) ? start.translate(0, 1) : proposedEnd;
      const severity = item.severity === 'error' ? vscode.DiagnosticSeverity.Error : item.severity === 'warning' ? vscode.DiagnosticSeverity.Warning : vscode.DiagnosticSeverity.Information;
      const diag = new vscode.Diagnostic(new vscode.Range(start, end), item.message, severity);
      diag.source = 'APX'; diag.code = item.code;
      grouped.set(item.path, [...(grouped.get(item.path) ?? []), diag]);
    }
    diagnosticGroups.set(root, grouped); renderDiagnostics();
    const found = report.discoveredTests ?? report.tests;
    const selectedFiles = new Set(report.files.filter(f => f.selected).map(f => path.resolve(f.path)));
    for (const [key, identity] of report.command === 'test' ? [] : identities) {
      if (identity.root === root && selectedFiles.has(path.resolve(identity.path))) { controller.items.delete(key); identities.delete(key); }
    }
    for (const test of report.command === 'test' ? [] : found) {
      if (!path.isAbsolute(test.path)) continue;
      const key = JSON.stringify([root, test.id]);
      const item = controller.createTestItem(key, test.id, vscode.Uri.file(test.path));
      item.description = `${path.basename(root)} · experimental`;
      item.range = new vscode.Range(Math.max(0, test.line - 1), 0, Math.max(0, test.line - 1), 0);
      controller.items.add(item); identities.set(key, {id: test.id, path: test.path, root});
    }
    output.appendLine(summaryLine(report));
    output.appendLine(`Scope: ${report.summary.selectedFiles} selected file(s); ${report.summary.files} loaded. Assurance: ${report.assurance}. Exit: ${report.exitCode}.`);
    for (const d of report.diagnostics) output.appendLine(`${d.severity.toUpperCase()} ${d.code} ${d.path ? `${d.path}:${d.line}:${d.column}` : ''} ${d.message}`);
    for (const t of report.tests) output.appendLine(`${t.status.toUpperCase()} ${t.id}: ${t.message || '(no message)'}`);
    for (const text of report.limitations) output.appendLine(`LIMITATION: ${text}`);
  }

  async function trust(): Promise<boolean> {
    if (vscode.workspace.isTrusted) return true;
    await vscode.window.showWarningMessage('APX requires a trusted workspace before running its native engine.');
    return false;
  }

  async function projectRoot(target: string): Promise<string> {
    const info = await fs.stat(target);
    const initial = info.isDirectory() ? target : path.dirname(target);
    let cursor = initial;
    for (;;) {
      try { if ((await fs.stat(path.join(cursor, 'sfdx-project.json'))).isFile()) return cursor; } catch { /* continue ancestors */ }
      const parent = path.dirname(cursor); if (parent === cursor) break; cursor = parent;
    }
    return vscode.workspace.getWorkspaceFolder(vscode.Uri.file(target))?.uri.fsPath ?? initial;
  }

  async function groupsFor(uris: readonly vscode.Uri[]): Promise<Group[]> {
    if (!uris.length) throw new Error('Choose an Apex class, folder or workspace first.');
    if (uris.some(u => u.scheme !== 'file')) throw new Error('Only files on the extension host filesystem are supported.');
    const groups = new Map<string, string[]>();
    for (const target of normalizeTargets(uris.map(u => u.fsPath))) {
      const root = await projectRoot(target);
      groups.set(root, [...(groups.get(root) ?? []), target]);
    }
    return [...groups].map(([root, paths]) => ({root, paths}));
  }

  async function saveRelevant(groups: Group[]): Promise<boolean> {
    const environments = groups.map(g => {
      const configured = vscode.workspace.getConfiguration('apx', vscode.Uri.file(g.root)).get<string>('environmentPath', '').trim();
      return configured ? path.resolve(g.root, configured) : undefined;
    });
    const dirty = vscode.workspace.textDocuments.filter(d => d.isDirty && d.uri.scheme === 'file' &&
      ((/\.(cls|trigger|xml|dwl|resource)$/i.test(d.fileName) || path.basename(d.fileName) === 'sfdx-project.json') &&
      groups.some(g => containedBy(g.root, d.fileName)) || environments.includes(path.resolve(d.fileName))));
    if (!dirty.length) return true;
    const answer = await vscode.window.showWarningMessage(`Save ${dirty.length} changed source, metadata or environment file(s) before validation? Dependency files are included.`, {modal: true}, 'Save and Continue');
    if (answer !== 'Save and Continue') return false;
    for (const doc of dirty) if (!await doc.save()) throw new Error(`Could not save ${doc.fileName}. Validation did not run.`);
    return true;
  }

  async function request(command: Command, group: Group, token: vscode.CancellationToken, filter?: string): Promise<ValidationReport | undefined> {
    active.get(group.root)?.abort();
    const abort = new AbortController(); active.set(group.root, abort);
    const generation = ledger.next(group.root);
    // Workspace-wide watchers do not observe folders chosen outside the workspace.
    // These request-scoped watchers are disposed with the request and cannot grow
    // indefinitely as a user validates different external projects.
    const scopedWatchers: vscode.Disposable[] = [];
    const workspaceRoots = (vscode.workspace.workspaceFolders ?? []).filter(f => f.uri.scheme === 'file').map(f => f.uri.fsPath);
    if (needsScopedWatcher(group.root, workspaceRoots)) {
      const changed = (uri: vscode.Uri): void => { if (ledger.current(group.root, generation)) invalidate(uri); };
      for (const pattern of ['**/*.{cls,trigger,xml,dwl,resource}', 'sfdx-project.json']) {
        const watcher = vscode.workspace.createFileSystemWatcher(new vscode.RelativePattern(group.root, pattern));
        scopedWatchers.push(watcher, watcher.onDidChange(changed), watcher.onDidCreate(changed), watcher.onDidDelete(changed));
      }
    }
    const cancellation = token.onCancellationRequested(() => abort.abort());
    if (token.isCancellationRequested) abort.abort();
    output.appendLine(`\n${command.toUpperCase()} ${group.paths.join(', ')}\nProject: ${group.root}`);
    try {
      const config = vscode.workspace.getConfiguration('apx', vscode.Uri.file(group.root));
      const environmentSetting = config.get<string>('environmentPath', '').trim();
      const environmentPath = environmentSetting ? path.resolve(group.root, environmentSetting) : undefined;
      for (const disposable of environmentWatchers.get(group.root) ?? []) disposable.dispose();
      environmentWatchers.delete(group.root); environmentFiles.delete(group.root);
      if (environmentPath) {
        environmentFiles.set(group.root, environmentPath);
        const watcher = vscode.workspace.createFileSystemWatcher(new vscode.RelativePattern(path.dirname(environmentPath), path.basename(environmentPath)));
        environmentWatchers.set(group.root, [watcher, watcher.onDidChange(invalidate), watcher.onDidCreate(invalidate), watcher.onDidDelete(invalidate)]);
        output.appendLine(`Local environment: ${environmentPath}`);
      }
      const explicit = config.get<string>('enginePath', '').trim();
      const enginePath = await resolveBundledEngine(context.extensionPath, explicit || undefined);
      const timeout = Math.max(1, Math.min(600, config.get<number>('timeoutSeconds', 60))) * 1000;
      const report = await runEngine({enginePath, command, paths: group.paths, projectRoot: group.root, filter, environmentPath, signal: abort.signal, timeoutMs: timeout, maxOutputBytes: 16 * 1024 * 1024});
      if (!ledger.current(group.root, generation) || abort.signal.aborted) return undefined;
      applyReport(report as ValidationReport, group.root);
      return report as ValidationReport;
    } catch (e) {
      if (abort.signal.aborted || (e as {code?: string}).code === 'APX_CANCELLED') { output.appendLine('Run cancelled or superseded; no new result applied.'); return undefined; }
      throw e;
    } finally {
      for (const watcher of scopedWatchers) watcher.dispose();
      cancellation.dispose();
      if (active.get(group.root) === abort) active.delete(group.root);
    }
  }

  async function execute(command: Command, uris: readonly vscode.Uri[], notify = true): Promise<ValidationReport[]> {
    if (!await trust()) return [];
    const groups = await groupsFor(uris); if (!await saveRelevant(groups)) return [];
    return await vscode.window.withProgress({location: vscode.ProgressLocation.Notification, title: `APX ${command}`, cancellable: true}, async (progress, token) => {
      const reports: ValidationReport[] = [];
      for (const group of groups) {
        if (token.isCancellationRequested) break;
        progress.report({message: path.basename(group.root)});
        const report = await request(command, group, token); if (report) reports.push(report);
      }
      if (notify && reports.length) {
        const text = reports.length === 1 ? summaryLine(reports[0]) : `APX completed ${reports.length} project request(s). Review Output for per-project results and limitations.`;
        const bad = reports.some(r => r.exitCode !== 0 && r.exitCode !== 4);
        const notification = bad ? vscode.window.showErrorMessage(text, 'Show Output') : reports.some(r => r.exitCode === 4) ? vscode.window.showWarningMessage(text, 'Show Output') : vscode.window.showInformationMessage(text, 'Show Output');
        void notification.then(action => { if (action) output.show(); });
      }
      return reports;
    });
  }

  const workspaceUris = (): vscode.Uri[] => (vscode.workspace.workspaceFolders ?? []).map(f => f.uri);
  const selectedUris = (uri?: vscode.Uri, uris?: vscode.Uri[]): vscode.Uri[] => uris?.length ? uris : uri ? [uri] : vscode.window.activeTextEditor ? [vscode.window.activeTextEditor.document.uri] : [];
  register('apx.validateSelection', (uri?: vscode.Uri, uris?: vscode.Uri[]) => execute('validate', selectedUris(uri, uris)));
  register('apx.checkSelection', (uri?: vscode.Uri, uris?: vscode.Uri[]) => execute('check', selectedUris(uri, uris)));
  register('apx.validateActiveFile', () => execute('validate', selectedUris()));
  register('apx.validateWorkspace', () => execute('validate', workspaceUris()));
  register('apx.validateFolder', async () => {
    const result = await vscode.window.showOpenDialog({canSelectFiles: false, canSelectFolders: true, canSelectMany: true, openLabel: 'Validate Folder'});
    if (result) await execute('validate', result);
  });
  register('apx.refreshTests', async () => { controller.items.replace([]); identities.clear(); await execute('check', workspaceUris(), false); });
  register('apx.showOutput', () => output.show());
  register('apx.showSfInstructions', async () => {
    const plugin = path.join(context.extensionPath, 'sf-plugin');
    const quoted = process.platform === 'win32' ? `'${plugin.replace(/'/g, "''")}'` : `'${plugin.replace(/'/g, `'"'"'`)}'`;
    const content = `# APX Salesforce CLI integration\n\nThe extension runs locally without Salesforce CLI. To use the same engine from an existing SF CLI installation, link the bundled plugin once.\n\nRun this command in ${process.platform === 'win32' ? 'PowerShell' : 'your shell'}:\n\n\`\`\`\nsf plugins link ${quoted}\n\`\`\`\n\nThen run:\n\n\`\`\`\nsf apx check --source-dir force-app/main/default/classes\nsf apx validate --source-dir force-app/main/default/classes\n\`\`\`\n\nUse \`sf apx validate --help\` for all options. No org connection is used. The preview engine reports unsupported behavior and returns exit 4 for unqualified local execution, including successful local assertions.\n\nAfter updating or uninstalling the extension, relink the current plugin location or remove it with \`sf plugins unlink @apx/sf-plugin\`. The extension never modifies your SF CLI installation automatically.\n`;
    const document = await vscode.workspace.openTextDocument({language: 'markdown', content});
    await vscode.window.showTextDocument(document, {preview: true});
  });
  controller.resolveHandler = async () => { if (!controller.items.size && workspaceUris().length) await execute('check', workspaceUris(), false); };
  controller.refreshHandler = async () => { controller.items.replace([]); identities.clear(); await execute('check', workspaceUris(), false); };
  controller.createRunProfile('Run locally (experimental)', vscode.TestRunProfileKind.Run, async (req, token) => {
    const run = controller.createTestRun(req, 'APX experimental execution');
    const excluded = new Set((req.exclude ?? []).map(t => t.id));
    const chosen: vscode.TestItem[] = [];
    if (req.include) chosen.push(...req.include); else controller.items.forEach(item => chosen.push(item));
    // Saving can invalidate discovery entries. Keep the user's exact selected identities
    // before any save prompt rather than rereading the mutable discovery map afterward.
    const selected = snapshotSelectedTests(chosen, identities, excluded);
    const tests = selected.map(s => s.item);
    const finished = new Set<string>();
    for (const item of tests) run.enqueued(item);
    try {
      if (!await trust()) return;
      const groups = await groupsFor(tests.flatMap(t => t.uri ? [t.uri] : []));
      if (!await saveRelevant(groups)) return;
      for (const {item, identity} of selected) {
        if (token.isCancellationRequested) { run.skipped(item); finished.add(item.id); continue; }
        run.started(item);
        const report = await request('test', {root: identity.root, paths: [identity.path]}, token, identity.id);
        if (!report) { run.errored(item, new vscode.TestMessage('APX execution cancelled or superseded.')); finished.add(item.id); break; }
        const result = report.tests.find(t => t.id === identity.id);
        if (!result) { run.errored(item, new vscode.TestMessage('The engine returned no terminal result for this selected test. See APX Output.')); finished.add(item.id); continue; }
        const message = result.status === 'passed' ? 'Local assertions passed under experimental execution. This is an advisory result, not qualified Salesforce behavior; shown as skipped to avoid a false green pass.' : result.message;
        run.appendOutput(`${message}\r\n`, item.uri && item.range ? new vscode.Location(item.uri, item.range) : undefined, item);
        switch (testPresentation(result)) {
          case 'failed': run.failed(item, new vscode.TestMessage(message), result.durationMs); break;
          case 'errored': run.errored(item, new vscode.TestMessage(message), result.durationMs); break;
          case 'skipped': run.skipped(item); break;
        }
        finished.add(item.id);
      }
    } catch (e) { run.appendOutput(`APX error: ${errorText(e)}\r\n`); for (const item of tests) if (!finished.has(item.id)) { run.errored(item, new vscode.TestMessage(errorText(e))); finished.add(item.id); } }
    finally { for (const item of tests) if (!finished.has(item.id)) run.skipped(item); run.end(); }
  }, true);

  const invalidate = (uri: vscode.Uri): void => {
    if (uri.scheme !== 'file') return;
    for (const [key, identity] of identities) if (path.resolve(identity.path) === path.resolve(uri.fsPath)) {
      controller.items.delete(key); identities.delete(key);
    }
    const roots = new Set([...active.keys(), ...diagnosticGroups.keys()]);
    for (const root of roots) if (containedBy(root, uri.fsPath) || environmentFiles.get(root) === path.resolve(uri.fsPath)) {
      ledger.next(root); active.get(root)?.abort(); diagnosticGroups.delete(root);
    }
    renderDiagnostics();
  };
  context.subscriptions.push(vscode.workspace.onDidChangeTextDocument(e => { if (/\.(cls|trigger|xml|dwl|resource)$/i.test(e.document.fileName) || path.basename(e.document.fileName) === 'sfdx-project.json' || [...environmentFiles.values()].includes(path.resolve(e.document.fileName))) invalidate(e.document.uri); }));
  context.subscriptions.push(vscode.workspace.onDidChangeConfiguration(e => {
    if (!e.affectsConfiguration('apx')) return;
    for (const root of new Set([...active.keys(), ...diagnosticGroups.keys()])) { ledger.next(root); active.get(root)?.abort(); diagnosticGroups.delete(root); }
    renderDiagnostics();
  }));
  for (const pattern of ['**/*.{cls,trigger,xml,dwl,resource}', '**/sfdx-project.json']) {
    const watcher = vscode.workspace.createFileSystemWatcher(pattern);
    context.subscriptions.push(watcher, watcher.onDidChange(invalidate), watcher.onDidCreate(invalidate), watcher.onDidDelete(invalidate));
  }
  context.subscriptions.push({dispose: () => { for (const abort of active.values()) abort.abort(); active.clear(); for (const watchers of environmentWatchers.values()) for (const disposable of watchers) disposable.dispose(); environmentWatchers.clear(); }});
}

export function deactivate(): void { for (const abort of active.values()) abort.abort(); active.clear(); }
