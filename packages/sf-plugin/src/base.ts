import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { Flags, SfCommand } from '@salesforce/sf-plugins-core';
import { EngineRunError, resolveBundledEngine, runEngine, type EngineCommand, type EngineReport } from '@apx/runner';

export abstract class ApxCommand extends SfCommand<EngineReport> {
  public static requiresProject = false;
  public static readonly flags = {
    'source-dir': Flags.string({ char: 'd', summary: 'Apex class files or folders to select (repeat for multiple paths).', multiple: true }),
    project: Flags.directory({ char: 'p', summary: 'Explicit project root (otherwise discover the enclosing Salesforce project).' }),
    filter: Flags.string({ summary: 'Exact Class.method or supported test filter passed to the engine.' }),
    environment: Flags.string({ summary: 'Explicit local environment JSON (relative to --project or current directory). No org connection.' }),
    engine: Flags.file({ summary: 'Absolute path to a native APX executable (defaults to bundled engine).', exists: true }),
    'timeout-ms': Flags.integer({ summary: 'Host deadline in milliseconds.', default: 120_000, min: 1, max: 3_600_000 }),
  };

  protected async execute(command: EngineCommand): Promise<EngineReport> {
    const { flags } = await this.parse(ApxCommand);
    const packageRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..');
    const controller = new AbortController();
    const interrupt = (): void => controller.abort();
    process.once('SIGINT', interrupt);
    try {
      const enginePath = await resolveBundledEngine(packageRoot, flags.engine);
      const report = await runEngine({
        enginePath, command, paths: flags['source-dir'] ?? ['.'], projectRoot: flags.project,
        filter: flags.filter, environmentPath: flags.environment, timeoutMs: flags['timeout-ms'], signal: controller.signal,
      });
      process.exitCode = report.exitCode;
      this.printReport(report);
      return report;
    } catch (error) {
      if (error instanceof EngineRunError) this.error(error.message, { code: error.code, exit: error.exitCode });
      throw error;
    } finally {
      process.removeListener('SIGINT', interrupt);
    }
  }

  private printReport(report: EngineReport): void {
    if (this.jsonEnabled()) return;
    this.log(`APX ${report.engineVersion} | ${report.command} | ${report.status}`);
    this.log(`Assurance: ${report.assurance}`);
    this.log(`Selected ${report.summary.selectedFiles} / ${report.summary.files} loaded files; ${report.summary.tests} tests.`);
    for (const diagnostic of report.diagnostics) {
      const at = diagnostic.path ? `${diagnostic.path}:${diagnostic.line}:${diagnostic.column}: ` : '';
      this.log(`${at}${diagnostic.severity} ${diagnostic.code}: ${diagnostic.message}`);
    }
    for (const test of report.tests) this.log(`${test.status.toUpperCase()} ${test.id}${test.message ? `: ${test.message}` : ''}`);
    for (const limitation of report.limitations) this.log(`Scope: ${limitation}`);
    if (report.status === 'preview_passed') this.log('Preview execution completed; exit 4 is intentional. This is not Salesforce deployment validation.');
  }

  protected toSuccessJson(result: EngineReport): SfCommand.Json<EngineReport> {
    return { ...super.toSuccessJson(result), status: result.exitCode };
  }
}
