@IsTest
private class AxtikQueryRowsPositive {
    @IsTest static void sample() {
        List<Account> rows = new List<Account>();
        for (Integer i = 0; i < 1000; i++) rows.add(new Account(Name='Query boundary'));
        insert as system rows;
        Integer queried = 0;
        for (Integer i = 0; i < 50; i++) {
            List<Account> result = [SELECT Id FROM Account WITH SYSTEM_MODE];
            queried += result.size();
        }
        System.assertEquals(50000, queried);
    }
}
