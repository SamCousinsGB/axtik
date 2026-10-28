import { ApxCommand } from '../../base.js';
import type { EngineReport } from '@apx/runner';

export default class Test extends ApxCommand {
  public static readonly summary = 'Run discovered Apex tests with the local preview evaluator.';
  public static readonly description = 'Tests use fresh local state. Only the implemented subset executes; unsupported behavior blocks rather than returning default values. Passing preview execution returns exit 4 and is not Salesforce conformance certification.';
  public static readonly examples = ['sf apx test --source-dir force-app/main/default/classes --filter AccountTest.createsAccount --json'];
  public async run(): Promise<EngineReport> { return this.execute('test'); }
}
