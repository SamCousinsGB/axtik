import { spawn } from 'node:child_process';
import { createHash } from 'node:crypto';
import { access, opendir, lstat, realpath, readFile, open } from 'node:fs/promises';
import { constants, createReadStream } from 'node:fs';
import { isAbsolute, join, resolve, relative, sep } from 'node:path';
import { validProvenance, type ReportProvenance, type TestProvenance } from './provenance.js';
export { describeTestProvenance } from './provenance.js';
export type { ReportProvenance, TestProvenance } from './provenance.js';

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
export interface AssertionCounts { executed: number; passed: number; failed: number }
export interface InputDigest { path: string; sha256: string }
export interface DirectoryDigest extends InputDigest { includeIgnored: boolean }
export interface EngineTest extends DiscoveredTest {
  status: 'passed' | 'failed' | 'blocked' | 'errored' | 'not_run';
  message: string;
  durationMs: number;
  assertions: AssertionCounts;
  setupAssertions: AssertionCounts;
  provenance?: TestProvenance;
}
export interface EngineReport {
  schemaVersion: 2;
  engineVersion: string;
  command: EngineCommand;
  status: 'checked' | 'preview_passed' | 'failed' | 'blocked' | 'errored';
  assurance: 'syntax-and-declarations' | 'experimental-local-execution';
  exitCode: number;
  projectRoot: string;
  inputs: InputDigest[];
  inputDirectories: DirectoryDigest[];
  inputFingerprint: string;
  files: Array<{ path: string; selected: boolean; classes: string[] }>;
  diagnostics: EngineDiagnostic[];
  tests: EngineTest[];
  discoveredTests?: DiscoveredTest[];
  summary: {
    files: number; selectedFiles: number; tests: number; passed: number;
    failed: number; blocked: number; errored: number; durationMs: number;
  };
  limitations: string[];
  provenance?: ReportProvenance;
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
const count = (value: unknown): value is number => number(value) && Number.isSafeInteger(value);
const hash = (value: unknown): value is string => string(value) && /^[a-f0-9]{64}$/.test(value);
const assertionCounts = (value: unknown): value is AssertionCounts => object(value) && count(value.executed) && count(value.passed) && count(value.failed) && value.executed === value.passed + value.failed;
const strings = (value: unknown): value is string[] => Array.isArray(value) && value.every(string);
const oneOf = (value: unknown, choices: readonly string[]): boolean => string(value) && choices.includes(value);
const discovered = (value: unknown): boolean => object(value) && string(value.id) && string(value.path) && position(value.line);

/** Reject partial, wrong-version and contradictory reports before a client displays results. */
export function validateReport(value: unknown, expectedCommand?: EngineCommand): EngineReport {
  if (!object(value) || value.schemaVersion !== 2 || !string(value.engineVersion) ||
      !oneOf(value.command, ['check', 'validate', 'test']) || (expectedCommand && value.command !== expectedCommand) ||
      !oneOf(value.status, ['checked', 'preview_passed', 'failed', 'blocked', 'errored']) ||
      !oneOf(value.assurance, ['syntax-and-declarations', 'experimental-local-execution']) ||
      !number(value.exitCode) || ![0, 1, 2, 3, 4, 130].includes(value.exitCode) || !string(value.projectRoot) || !Array.isArray(value.inputs) || !Array.isArray(value.inputDirectories) || !hash(value.inputFingerprint) ||
      !strings(value.limitations) || !Array.isArray(value.files) || !Array.isArray(value.diagnostics) ||
      !Array.isArray(value.tests) || !object(value.summary)) {
    throw new EngineRunError('APX_PROTOCOL', 'The APX engine returned an incomplete or incompatible report.');
  }
  if (![...value.inputs,...value.inputDirectories].every(i => object(i) && string(i.path) && isAbsolute(i.path) && hash(i.sha256)) ||
      !value.inputDirectories.every(i => object(i) && typeof i.includeIgnored === 'boolean') ||
      !value.files.every(f => object(f) && string(f.path) && typeof f.selected === 'boolean' && strings(f.classes)) ||
      !value.diagnostics.every(d => object(d) && string(d.code) && string(d.message) && string(d.path) &&
        oneOf(d.severity, ['error', 'warning', 'information']) && position(d.line) && position(d.column) &&
        position(d.endLine) && position(d.endColumn)) ||
      !value.tests.every(t => discovered(t) && object(t) && oneOf(t.status, ['passed', 'failed', 'blocked', 'errored', 'not_run']) &&
        string(t.message) && number(t.durationMs) && assertionCounts(t.assertions) && assertionCounts(t.setupAssertions)) ||
      (value.discoveredTests !== undefined && (!Array.isArray(value.discoveredTests) || !value.discoveredTests.every(discovered)))) {
    throw new EngineRunError('APX_PROTOCOL', 'The APX report contains malformed file, diagnostic or test entries.');
  }
  for (const key of ['files', 'selectedFiles', 'tests', 'passed', 'failed', 'blocked', 'errored', 'durationMs']) {
    if (!number(value.summary[key])) throw new EngineRunError('APX_PROTOCOL', `Invalid report summary field: ${key}.`);
  }
  const result = value as unknown as EngineReport;
  if (!validProvenance(value)) {
    throw new EngineRunError('APX_PROTOCOL', 'The APX capability inventory is incomplete, contradictory, or claims unsupported conformance.');
  }
  if ([result.inputs,result.inputDirectories].some(inputs => new Set(inputs.map(i => process.platform === 'win32' ? i.path.toLowerCase() : i.path)).size !== inputs.length) || inputFingerprint(result.inputs,result.inputDirectories) !== result.inputFingerprint) {
    throw new EngineRunError('APX_PROTOCOL', 'The APX input manifest contains duplicates or an inconsistent fingerprint.');
  }
  if (result.tests.some(t => (t.status === 'not_run' && (t.assertions.executed !== 0 || t.setupAssertions.executed !== 0)) ||
      (t.status === 'passed' && (t.assertions.failed !== 0 || t.setupAssertions.failed !== 0)))) {
    throw new EngineRunError('APX_PROTOCOL', 'The APX assertion counts contradict the test outcomes.');
  }
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

/** Native order is canonical; serialize only contracted keys, in the protocol order. */
export function inputFingerprint(inputs: readonly InputDigest[], directories: readonly DirectoryDigest[]): string {
  const canonical = (items: readonly InputDigest[]): InputDigest[] => items.map(input => ({path:input.path,sha256:input.sha256}));
  return createHash('sha256').update(JSON.stringify({files:canonical(inputs),directories:directories.map(input => ({path:input.path,sha256:input.sha256,includeIgnored:input.includeIgnored}))}),'utf8').digest('hex');
}

export const ignoredInputNames = new Set(['.git','node_modules','target','.sf','.sfdx','.apx','dist']);

export async function directoryFingerprint(directory: string, maximumEntries = 100_000, signal?: AbortSignal, includeIgnored = false): Promise<{sha256:string;entries:number}> {
  const entries: Array<{name:string;kind:string}> = [];
  for await (const entry of await opendir(directory)) {
    if (signal?.aborted) throw new EngineRunError('APX_CANCELLED', 'APX operation cancelled.', 130);
    if (!includeIgnored && ignoredInputNames.has(entry.name)) continue;
    if (entries.length >= maximumEntries) throw new EngineRunError('APX_INPUT_LIMIT', 'Input directory verification exceeds 100000 entries.', 4);
    entries.push({name:entry.name,kind:entry.isSymbolicLink()?'symlink':entry.isFile()?'file':entry.isDirectory()?'directory':'other'});
  }
  entries.sort((a,b) => Buffer.compare(Buffer.from(a.name,'utf8'),Buffer.from(b.name,'utf8')));
  return {sha256:createHash('sha256').update(JSON.stringify(entries),'utf8').digest('hex'),entries:entries.length};
}

/** Verify the actual consumed bytes, including arbitrary static-resource formats. */
export async function verifyReportInputs(report: Pick<EngineReport, 'inputs' | 'inputDirectories' | 'inputFingerprint'>, signal?: AbortSignal): Promise<void> {
  if (signal?.aborted) throw new EngineRunError('APX_CANCELLED', 'APX operation cancelled.', 130);
  if (inputFingerprint(report.inputs,report.inputDirectories) !== report.inputFingerprint) throw new EngineRunError('APX_PROTOCOL', 'The APX input fingerprint is inconsistent.');
  let next = 0;
  let verifiedBytes = 0;
  await Promise.all(Array.from({length:Math.min(4,report.inputs.length)}, async () => {
    for (;;) {
      if (signal?.aborted) throw new EngineRunError('APX_CANCELLED', 'APX operation cancelled.', 130);
      const input = report.inputs[next++];
      if (!input) return;
      const digest = createHash('sha256');
      try {
        const stream = createReadStream(input.path, {signal});
        for await (const chunk of stream) {
          verifiedBytes += (chunk as Buffer).length;
          if (verifiedBytes > 256 * 1024 * 1024) { stream.destroy(); throw new EngineRunError('APX_INPUT_LIMIT', 'Consumed input verification exceeds the 256 MiB host bound.', 4); }
          digest.update(chunk);
        }
      } catch (error) {
        if (signal?.aborted) throw new EngineRunError('APX_CANCELLED', 'APX operation cancelled.', 130);
        if (error instanceof EngineRunError) throw error;
        throw new EngineRunError('APX_INPUT_CHANGED', `A consumed input is unavailable: ${input.path}. Run validation again.`, 4);
      }
      if (digest.digest('hex') !== input.sha256) throw new EngineRunError('APX_INPUT_CHANGED', `A consumed input changed: ${input.path}. Run validation again.`, 4);
    }
  }));
  let entries = 0;
  for (const directory of report.inputDirectories) {
    if (signal?.aborted) throw new EngineRunError('APX_CANCELLED', 'APX operation cancelled.', 130);
    let observed;
    try { observed = await directoryFingerprint(directory.path,100_000-entries,signal,directory.includeIgnored); }
    catch (error) { if(error instanceof EngineRunError) throw error; throw new EngineRunError('APX_INPUT_CHANGED', `An input directory is unavailable: ${directory.path}. Run validation again.`, 4); }
    entries += observed.entries;
    if (entries > 100_000) throw new EngineRunError('APX_INPUT_LIMIT', 'Input directory verification exceeds 100000 entries.', 4);
    if (observed.sha256 !== directory.sha256) throw new EngineRunError('APX_INPUT_CHANGED', `Input directory entries changed: ${directory.path}. Run validation again.`, 4);
  }
}

export function bundledEngineTarget(platform: string = process.platform, arch: string = process.arch): string {
  if (!['win32', 'darwin', 'linux'].includes(platform) || !['x64', 'arm64'].includes(arch)) {
    throw new EngineRunError('APX_PLATFORM_UNSUPPORTED', `Axtik supports Windows, macOS and Linux on x64 or ARM64. This extension host is ${platform}-${arch}.`, 2);
  }
  return `${platform}-${arch}`;
}

/** Repair an installer's lost Unix execute bit only for a hash-verified bundle. */
async function repairBundledPermissions(packageRoot: string, enginePath: string, target: string): Promise<void> {
  const info = JSON.parse(await readFile(join(packageRoot, 'bin', target, 'engine.json'), 'utf8')) as Record<string, unknown>;
  const manifest = JSON.parse(await readFile(join(packageRoot, 'package.json'), 'utf8')) as Record<string, unknown>;
  if (info.schemaVersion !== 1 || info.target !== target || info.version !== manifest.version || !hash(info.sha256) || !count(info.bytes) || Number(info.bytes) > 128 * 1024 * 1024) throw new Error('Invalid bundled engine receipt');
  const handle = await open(enginePath, constants.O_RDONLY | constants.O_NOFOLLOW);
  try {
    const stat = await handle.stat();
    if (!stat.isFile() || stat.size !== info.bytes) throw new Error('Engine size differs from its receipt');
    const digest = createHash('sha256');
    let size = 0;
    for await (const chunk of handle.createReadStream({autoClose: false})) {
      size += chunk.length;
      if (size > Number(info.bytes)) throw new Error('Engine changed during verification');
      digest.update(chunk);
    }
    if (size !== info.bytes || digest.digest('hex') !== info.sha256) throw new Error('Engine hash differs from its receipt');
    // Grant the current owner execution only; retain all other existing bits.
    await handle.chmod(stat.mode | 0o100);
  } finally { await handle.close(); }
}

export async function resolveBundledEngine(packageRoot: string, explicitPath?: string): Promise<string> {
  if (explicitPath && !isAbsolute(explicitPath)) {
    throw new EngineRunError('APX_ENGINE_PATH', 'The configured APX engine path must be absolute.', 2);
  }
  const target = explicitPath ? `${process.platform}-${process.arch}` : bundledEngineTarget();
  const enginePath = explicitPath ?? join(packageRoot, 'bin', target, process.platform === 'win32' ? 'apx.exe' : 'apx');
  if (!explicitPath) {
    try {
      for (const file of [join(packageRoot, 'bin'), join(packageRoot, 'bin', target), enginePath]) {
        if ((await lstat(file)).isSymbolicLink()) throw new Error('Bundled paths must not be symlinks');
      }
      if (!(await lstat(enginePath)).isFile()) throw new Error('Bundled engine must be a regular file');
      const rel = relative(await realpath(packageRoot), await realpath(enginePath));
      if (isAbsolute(rel) || rel === '..' || rel.startsWith(`..${sep}`)) throw new Error('Bundled engine escapes the extension');
    } catch {
      throw new EngineRunError('APX_ENGINE_MISSING', `The bundled Axtik engine for ${target} is missing or damaged. Reinstall the complete Axtik extension.`, 2);
    }
  }
  try {
    await access(enginePath, process.platform === 'win32' ? constants.F_OK : constants.X_OK);
  } catch {
    if (!explicitPath && process.platform !== 'win32') {
      try {
        await repairBundledPermissions(packageRoot, enginePath, target);
        await access(enginePath, constants.X_OK);
        return enginePath;
      } catch {
        throw new EngineRunError('APX_ENGINE_PERMISSIONS', `The bundled Axtik engine cannot execute on ${target}. Reinstall the extension on a filesystem that permits executable files.`, 2);
      }
    }
    throw new EngineRunError('APX_ENGINE_MISSING', `The configured Axtik engine cannot be read or executed at ${enginePath}. Choose an executable for ${target}, or clear the override to use the bundled engine.`, 2);
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
    const verificationAbort = new AbortController();
    const stdout: Buffer[] = [];
    const stderr: Buffer[] = [];
    let bytes = 0;
    let stderrBytes = 0;
    let pendingError: EngineRunError | undefined;
    let settled = false;
    const finish = (error?: EngineRunError, report?: EngineReport): void => {
      if (settled) return;
      settled = true;
      verificationAbort.abort();
      clearTimeout(timer);
      options.signal?.removeEventListener('abort', onAbort);
      if (error) reject(error); else resolveResult(report!);
    };
    const stop = (error: EngineRunError): void => {
      if (pendingError || settled) return;
      pendingError = error;
      verificationAbort.abort();
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
    child.on('close', async (code, signal) => {
      if (pendingError) return finish(pendingError);
      const errText = Buffer.concat(stderr).toString('utf8');
      if (code === null) return finish(new EngineRunError('APX_ENGINE_EXIT', `APX terminated unexpectedly (${signal ?? 'no exit code'}).`, 3, errText));
      try {
        const report = validateReport(JSON.parse(Buffer.concat(stdout).toString('utf8')), options.command);
        if (report.exitCode !== code) throw new EngineRunError('APX_PROTOCOL', `APX report exit ${report.exitCode} does not match process exit ${code}.`, 3, errText);
        await verifyReportInputs(report, verificationAbort.signal);
        if (pendingError) return finish(pendingError);
        finish(undefined, report);
      } catch (error) {
        finish(pendingError ?? (error instanceof EngineRunError ? error : new EngineRunError('APX_PROTOCOL', 'APX did not produce a valid JSON report.', 3, errText)));
      }
    });
  });
}
