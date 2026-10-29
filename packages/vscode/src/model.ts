import * as path from 'node:path';
import { ignoredInputNames, type EngineReport, type EngineTest } from '../../runner/src/index';

/** Presentation logic deliberately has no VS Code dependency, so it is regression-tested. */
export type TestObservation = EngineTest;
export type ValidationReport = EngineReport;
export type TestIdentity = {id: string; path: string; root: string};

export type InstallRecovery = 'Open Engine Settings' | 'Download Extension' | 'View Supported Platforms';
export function engineFailurePresentation(error: unknown, usesOverride: boolean): {message: string; recovery?: InstallRecovery} {
  const message = error instanceof Error ? error.message : String(error);
  const code = typeof error === 'object' && error !== null && 'code' in error ? error.code : undefined;
  if (code === 'APX_PLATFORM_UNSUPPORTED') return {message, recovery: 'View Supported Platforms'};
  if (code === 'APX_ENGINE_PATH' || code === 'APX_ENGINE_MISSING' || code === 'APX_ENGINE_PERMISSIONS') {
    if (usesOverride) return {message: 'The configured engine could not be used. Clear apx.enginePath in Settings to use the engine included with Axtik.', recovery: 'Open Engine Settings'};
    return {message: 'The engine included with Axtik is unavailable. Reinstall the latest extension; no separate engine setup is required.', recovery: 'Download Extension'};
  }
  return {message};
}

export function snapshotSelectedTests<T extends {id: string}>(items: readonly T[], identities: ReadonlyMap<string, TestIdentity>, excluded: ReadonlySet<string>): {item: T; identity: TestIdentity}[] {
  return items.flatMap(item => {
    const identity = identities.get(item.id);
    return identity && !excluded.has(item.id) ? [{item, identity: {...identity}}] : [];
  });
}

export function containedBy(root: string, file: string): boolean {
  const relative = path.relative(path.resolve(root), path.resolve(file));
  return relative === '' || (!relative.startsWith(`..${path.sep}`) && relative !== '..' && !path.isAbsolute(relative));
}

export function normalizeTargets(targets: string[]): string[] {
  return [...new Set(targets.map(p => path.resolve(p)))];
}

export function needsScopedWatcher(root: string, workspaceRoots: readonly string[]): boolean {
  return !workspaceRoots.some(workspaceRoot => containedBy(workspaceRoot, root));
}

export function inputPathKey(file: string): string {
  const resolved = path.resolve(file);
  return process.platform === 'win32' ? resolved.toLowerCase() : resolved;
}

/** Keep live subscriptions for unchanged paths while replacing a dependency set. */
export class RetainedDisposables {
  private readonly entries = new Map<string, {dispose(): void}>();

  update(keys: Iterable<string>, create: (key: string) => {dispose(): void}): void {
    const next = new Set(keys);
    // Register additions first, so replacing a set does not drop existing coverage.
    for (const key of next) if (!this.entries.has(key)) this.entries.set(key, create(key));
    for (const [key, value] of this.entries) if (!next.has(key)) { value.dispose(); this.entries.delete(key); }
  }

  dispose(): void {
    for (const value of this.entries.values()) value.dispose();
    this.entries.clear();
  }
}

/** Unknown/new files may change the native directory inventory, regardless of extension. */
export function affectsProject(root: string, file: string, consumed: ReadonlySet<string> = new Set(), environment?: string, directories: ReadonlyMap<string,boolean> = new Map()): boolean {
  const key = inputPathKey(file);
  if (consumed.has(key) || (environment && key === inputPathKey(environment))) return true;
  if ([...directories].some(([directory,includeIgnored]) => includeIgnored && containedBy(directory,file))) return true;
  if (directories.has(inputPathKey(path.dirname(file))) && !ignoredInputNames.has(path.basename(file))) return true;
  return containedBy(root,file) && !path.relative(root,file).split(path.sep).some(part => ignoredInputNames.has(part));
}

export function assertionDescription(test: TestObservation): string {
  if (test.status === 'not_run') return 'not run';
  const counts = test.assertions;
  return `${counts.executed} test assertion(s): ${counts.passed} passed, ${counts.failed} failed; ${test.setupAssertions.executed} setup assertion(s)${counts.executed === 0 ? '; no test assertion executed' : ''}`;
}

export function testPresentation(test: TestObservation): 'failed' | 'errored' | 'skipped' {
  // A locally passed assertion is still unqualified preview execution.
  // VS Code has no advisory state; skipped + explicit output avoids a false green tick.
  if (test.status === 'failed') return 'failed';
  if (test.status === 'passed' || test.status === 'not_run') return 'skipped';
  return 'errored';
}

export function summaryLine(report: ValidationReport): string {
  if (report.command === 'check') {
    const result = report.status === 'checked' ? 'completed' : `reported ${report.status}`;
    return `APX: syntax/declaration checks ${result} for ${report.summary.selectedFiles} selected file(s). Tests were not run.`;
  }
  const s = report.summary;
  const assertions = report.tests.reduce((n,t) => n+t.assertions.executed,0);
  const zero = report.tests.filter(t => t.status !== 'not_run' && t.assertions.executed === 0).length;
  return `APX ${report.status}: ${s.tests} test(s), ${s.passed} local test passes, ${s.failed} failed, ${s.blocked} blocked, ${s.errored} engine errors; ${assertions} test assertion(s), ${zero} test(s) without assertions. Experimental assurance; no Salesforce certification.`;
}

export class RequestLedger {
  private readonly versions = new Map<string, number>();
  next(key: string): number {
    const value = (this.versions.get(key) ?? 0) + 1;
    this.versions.set(key, value);
    return value;
  }
  current(key: string, generation: number): boolean { return this.versions.get(key) === generation; }
}
