@IsTest
private class AxtikDmlRowsNegative {
    @IsTest static void sample() {
        List<Account> rows = new List<Account>();
        for (Integer i = 0; i < 10001; i++) rows.add(new Account(Name='DML boundary'));
        Boolean caught = false;
        try {
            insert as system rows;
        } catch (DmlException error) {
            caught = true;
        }
        // An engine that converts a governor failure to DmlException will pass.
        // The oracle must instead report an unhandled System.LimitException.
        System.assert(caught, 'A governor failure must terminate the transaction');
    }
}
