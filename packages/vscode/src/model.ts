import * as path from 'node:path';
import type { EngineReport, EngineTest } from '../../runner/src/index';

/** Presentation logic deliberately has no VS Code dependency, so it is regression-tested. */
export type TestObservation = EngineTest;
export type ValidationReport = EngineReport;
export type TestIdentity = {id: string; path: string; root: string};

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
  return `APX ${report.status}: ${s.tests} test(s), ${s.passed} local assertion passes, ${s.failed} failed, ${s.blocked} blocked, ${s.errored} engine errors. Experimental assurance; no Salesforce certification.`;
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
