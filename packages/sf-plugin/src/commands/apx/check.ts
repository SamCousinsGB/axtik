import { ApxCommand } from '../../base.js';
import type { EngineReport } from '@apx/runner';

export default class Check extends ApxCommand {
  public static readonly summary = 'Check Apex syntax and declarations locally without an org.';
  public static readonly description = 'Select class files or folders. Dependencies load from the project root. A successful check certifies only the reported syntax/declaration scope, not complete Apex semantics or deployment readiness.';
  public static readonly examples = [
    'sf apx check --source-dir force-app/main/default/classes --project .',
    'sf apx check --source-dir classes/Service.cls --source-dir classes/ServiceTest.cls --json',
  ];
  public async run(): Promise<EngineReport> { return this.execute('check'); }
}
