@IsTest
private class AxtikFieldPositive {
    @IsTest static void sample() {
        String name = '';
        for (Integer i = 0; i < 255; i++) name += 'a';
        Account row = new Account(Name=name);
        insert as system row;
        Account actual = [SELECT Name FROM Account WHERE Id=:row.Id WITH SYSTEM_MODE];
        System.assertEquals(255, actual.Name.length());
    }
}
