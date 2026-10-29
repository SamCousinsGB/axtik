@IsTest
private class AxtikDmlRowsPositive {
    @IsTest static void sample() {
        List<Account> rows = new List<Account>();
        for (Integer i = 0; i < 10000; i++) rows.add(new Account(Name='DML boundary'));
        insert as system rows;
        System.assertEquals(10000, [SELECT count() FROM Account WITH SYSTEM_MODE]);
    }
}
