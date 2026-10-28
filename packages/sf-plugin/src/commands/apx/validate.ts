import { ApxCommand } from '../../base.js';
import type { EngineReport } from '@apx/runner';

export default class Validate extends ApxCommand {
  public static readonly summary = 'Check selected Apex and run supported local tests in preview mode.';
  public static readonly description = 'Validation is offline experimental evaluation, not an org deployment. Unsupported semantics block explicitly. Preview successes retain exit code 4 until independently qualified; test assertion failures return 1.';
  public static readonly examples = [
    'sf apx validate --source-dir force-app/main/default/classes --project .',
    'sf apx validate --source-dir classes/ServiceTest.cls --filter ServiceTest.createsRecord --json',
  ];
  public async run(): Promise<EngineReport> { return this.execute('validate'); }
}
