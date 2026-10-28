@IsTest private class FailedAssertion {
    @IsTest static void deliberateFailure() {
        System.assertEquals(42, 41, 'Negative control must fail');
    }
}
