import { spawn } from 'node:child_process';
import { access } from 'node:fs/promises';
import { constants } from 'node:fs';
import { isAbsolute, join, resolve } from 'node:path';

export type EngineCommand = 'check' | 'validate' | 'test';
export interface EngineDiagnostic {
  code: string;
  severity: 'error' | 'warning' | 'information';
  message: string;
  path: string;
  line: number;
  column: number;
  endLine: number;
  endColumn: number;
}
export interface DiscoveredTest { id: string; path: string; line: number }
export interface EngineTest extends DiscoveredTest {
  status: 'passed' | 'failed' | 'blocked' | 'errored' | 'not_run';
  message: string;
  durationMs: number;
}
export interface EngineReport {
  schemaVersion: 1;
  engineVersion: string;
  command: EngineCommand;
  status: 'checked' | 'preview_passed' | 'failed' | 'blocked' | 'errored';
  assurance: 'syntax-and-declarations' | 'experimental-local-execution';
  exitCode: number;
  projectRoot: string;
  files: Array<{ path: string; selected: boolean; classes: string[] }>;
  diagnostics: EngineDiagnostic[];
  tests: EngineTest[];
  discoveredTests?: DiscoveredTest[];
  summary: {
    files: number; selectedFiles: number; tests: number; passed: number;
    failed: number; blocked: number; errored: number; durationMs: number;
  };
  limitations: string[];
}

export class EngineRunError extends Error {
  constructor(public readonly code: string, message: string, public readonly exitCode = 3, public readonly stderr = '') {
    super(message);
    this.name = 'EngineRunError';
  }
}

export interface RunEngineOptions {
  enginePath: string;
  command: EngineCommand;
  paths: string[];
  projectRoot?: string;
  filter?: string;
  environmentPath?: string;
  signal?: AbortSignal;
  timeoutMs?: number;
  maxOutputBytes?: number;
}

const object = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);
const string = (value: unknown): value is string => typeof value === 'string';
const number = (value: unknown): value is number => typeof value === 'number' && Number.isFinite(value) && value >= 0;
const position = (value: unknown): value is number => number(value) && Number.isInteger(value) && value >= 1;
const strings = (value: unknown): value is string[] => Array.isArray(value) && value.every(string);
const oneOf = (value: unknown, choices: readonly string[]): boolean => string(value) && choices.includes(value);
const discovered = (value: unknown): boolean => object(value) && string(value.id) && string(value.path) && position(value.line);

/** Reject partial, wrong-version and contradictory reports before a client displays results. */
export function validateReport(value: unknown, expectedCommand?: EngineCommand): EngineReport {
  if (!object(value) || value.schemaVersion !== 1 || !string(value.engineVersion) ||
      !oneOf(value.command, ['check', 'validate', 'test']) || (expectedCommand && value.command !== expectedCommand) ||
      !oneOf(value.status, ['checked', 'preview_passed', 'failed', 'blocked', 'errored']) ||
      !oneOf(value.assurance, ['syntax-and-declarations', 'experimental-local-execution']) ||
      !number(value.exitCode) || ![0, 1, 2, 3, 4, 130].includes(value.exitCode) || !string(value.projectRoot) ||
      !strings(value.limitations) || !Array.isArray(value.files) || !Array.isArray(value.diagnostics) ||
      !Array.isArray(value.tests) || !object(value.summary)) {
    throw new EngineRunError('APX_PROTOCOL', 'The APX engine returned an incomplete or incompatible report.');
  }
  if (!value.files.every(f => object(f) && string(f.path) && typeof f.selected === 'boolean' && strings(f.classes)) ||
      !value.diagnostics.every(d => object(d) && string(d.code) && string(d.message) && string(d.path) &&
        oneOf(d.severity, ['error', 'warning', 'information']) && position(d.line) && position(d.column) &&
        position(d.endLine) && position(d.endColumn)) ||
      !value.tests.every(t => discovered(t) && object(t) && oneOf(t.status, ['passed', 'failed', 'blocked', 'errored', 'not_run']) &&
        string(t.message) && number(t.durationMs)) ||
      (value.discoveredTests !== undefined && (!Array.isArray(value.discoveredTests) || !value.discoveredTests.every(discovered)))) {
    throw new EngineRunError('APX_PROTOCOL', 'The APX report contains malformed file, diagnostic or test entries.');
  }
  for (const key of ['files', 'selectedFiles', 'tests', 'passed', 'failed', 'blocked', 'errored', 'durationMs']) {
    if (!number(value.summary[key])) throw new EngineRunError('APX_PROTOCOL', `Invalid report summary field: ${key}.`);
  }
  const result = value as unknown as EngineReport;
  if (result.command !== 'check' && result.tests.some(t => t.status === 'not_run')) {
    throw new EngineRunError('APX_PROTOCOL', 'An execution report contains tests that were never accounted for.');
  }
  if (result.summary.files !== result.files.length || result.summary.selectedFiles !== result.files.filter(f => f.selected).length ||
      result.summary.tests !== result.tests.length || new Set(result.tests.map(t => t.id)).size !== result.tests.length) {
    throw new EngineRunError('APX_PROTOCOL', 'The APX report selection or test counts are inconsistent.');
  }
  for (const status of ['passed', 'failed', 'blocked', 'errored'] as const) {
    if (result.summary[status] !== result.tests.filter(t => t.status === status).length) {
      throw new EngineRunError('APX_PROTOCOL', `The APX report ${status} count is inconsistent.`);
    }
  }
  if ((result.status === 'checked' && (result.command !== 'check' || result.exitCode !== 0 || result.assurance !== 'syntax-and-declarations')) ||
      (result.status === 'preview_passed' && (result.command === 'check' || result.exitCode !== 4 || result.assurance !== 'experimental-local-execution')) ||
      (result.exitCode === 0 && result.status !== 'checked')) {
    throw new EngineRunError('APX_PROTOCOL', 'The APX report incorrectly labels preview execution as qualified success.');
  }
  if ((result.status === 'checked' && (result.diagnostics.some(d => d.severity === 'error') || result.tests.some(t => t.status !== 'not_run'))) ||
      (result.status === 'preview_passed' && (result.diagnostics.some(d => d.severity === 'error') || result.tests.some(t => t.status !== 'passed')))) {
    throw new EngineRunError('APX_PROTOCOL', 'The APX report success label contradicts its diagnostics or test outcomes.');
  }
  return result;
}

export async function resolveBundledEngine(packageRoot: string, explicitPath?: string): Promise<string> {
  if (explicitPath && !isAbsolute(explicitPath)) {
    throw new EngineRunError('APX_ENGINE_PATH', 'The configured APX engine path must be absolute.', 2);
  }
  const enginePath = explicitPath ?? join(packageRoot, 'bin', `${process.platform}-${process.arch}`, process.platform === 'win32' ? 'apx.exe' : 'apx');
  try {
    await access(enginePath, process.platform === 'win32' ? constants.F_OK : constants.X_OK);
  } catch {
    throw new EngineRunError('APX_ENGINE_MISSING', `No APX engine is available for ${process.platform}-${process.arch} at ${enginePath}. Install the matching native package or configure an absolute engine path.`, 2);
  }
  return enginePath;
}

/** Only an argv array reaches the native process. No shell, credential environment, or network fallback. */
export async function runEngine(options: RunEngineOptions): Promise<EngineReport> {
  if (options.signal?.aborted) throw new EngineRunError('APX_CANCELLED', 'APX operation cancelled.', 130);
  if (!isAbsolute(options.enginePath)) throw new EngineRunError('APX_ENGINE_PATH', 'APX engine path must be absolute.', 2);
  if (!['check', 'validate', 'test'].includes(options.command)) throw new EngineRunError('APX_ARGUMENT', 'Unknown APX command.', 2);
  const cwd = resolve(options.projectRoot ?? process.cwd());
  const args: string[] = [options.command, '--json'];
  if (options.projectRoot !== undefined) args.push('--project', cwd);
  if (options.filter !== undefined) args.push('--filter', options.filter);
  if (options.environmentPath !== undefined) args.push('--environment', resolve(cwd, options.environmentPath));
  args.push(...options.paths.map(path => resolve(cwd, path)));
  const timeoutMs = options.timeoutMs ?? 120_000;
  const maxOutputBytes = options.maxOutputBytes ?? 16 * 1024 * 1024;
  if (!Number.isFinite(timeoutMs) || timeoutMs < 1 || !Number.isFinite(maxOutputBytes) || maxOutputBytes < 1) {
    throw new EngineRunError('APX_ARGUMENT', 'Timeout and output limits must be positive finite numbers.', 2);
  }
  const env: NodeJS.ProcessEnv = { NO_COLOR: '1' };
  for (const key of ['SystemRoot', 'WINDIR', 'TEMP', 'TMP']) if (process.env[key]) env[key] = process.env[key];
  return new Promise<EngineReport>((resolveResult, reject) => {
    const child = spawn(options.enginePath, args, { cwd, env, shell: false, windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'] });
    const stdout: Buffer[] = [];
    const stderr: Buffer[] = [];
    let bytes = 0;
    let stderrBytes = 0;
    let pendingError: EngineRunError | undefined;
    let settled = false;
    const finish = (error?: EngineRunError, report?: EngineReport): void => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      options.signal?.removeEventListener('abort', onAbort);
      if (error) reject(error); else resolveResult(report!);
    };
    const stop = (error: EngineRunError): void => {
      if (pendingError || settled) return;
      pendingError = error;
      child.kill('SIGKILL');
    };
    const onAbort = (): void => stop(new EngineRunError('APX_CANCELLED', 'APX operation cancelled.', 130));
    const timer = setTimeout(() => stop(new EngineRunError('APX_TIMEOUT', `APX exceeded the ${timeoutMs} ms host deadline.`)), timeoutMs);
    options.signal?.addEventListener('abort', onAbort, { once: true });
    if (options.signal?.aborted) onAbort();
    child.stdout.on('data', (data: Buffer) => {
      bytes += data.length;
      if (bytes > maxOutputBytes) stop(new EngineRunError('APX_OUTPUT_LIMIT', 'APX exceeded the report size limit.'));
      else stdout.push(data);
    });
    child.stderr.on('data', (data: Buffer) => {
      bytes += data.length;
      stderrBytes += data.length;
      if (stderrBytes <= 256 * 1024) stderr.push(data);
      if (bytes > maxOutputBytes) stop(new EngineRunError('APX_OUTPUT_LIMIT', 'APX exceeded the output size limit.'));
    });
    child.on('error', error => finish(pendingError ?? new EngineRunError('APX_ENGINE_START', `Cannot start APX: ${error.message}`)));
    child.on('close', (code, signal) => {
      if (pendingError) return finish(pendingError);
      const errText = Buffer.concat(stderr).toString('utf8');
      if (code === null) return finish(new EngineRunError('APX_ENGINE_EXIT', `APX terminated unexpectedly (${signal ?? 'no exit code'}).`, 3, errText));
      try {
        const report = validateReport(JSON.parse(Buffer.concat(stdout).toString('utf8')), options.command);
        if (report.exitCode !== code) throw new EngineRunError('APX_PROTOCOL', `APX report exit ${report.exitCode} does not match process exit ${code}.`, 3, errText);
        finish(undefined, report);
      } catch (error) {
        finish(error instanceof EngineRunError ? error : new EngineRunError('APX_PROTOCOL', 'APX did not produce a valid JSON report.', 3, errText));
      }
    });
  });
}
