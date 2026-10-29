@IsTest
private class AxtikSharingNegative {
    @IsTest static void sample() {
        List<User> actors = [SELECT Id, IsActive FROM User WHERE Alias='axtikact' WITH SYSTEM_MODE];
        List<User> owners = [SELECT Id, IsActive FROM User WHERE Alias='axtikown' WITH SYSTEM_MODE];
        System.assertEquals(1, actors.size(), 'Exactly one comparison actor is required');
        System.assertEquals(1, owners.size(), 'Exactly one comparison owner is required');
        System.assert(actors[0].IsActive && owners[0].IsActive);
        System.assertNotEquals(actors[0].Id, owners[0].Id);
        Account row = new Account(Name='Original', OwnerId=owners[0].Id);
        insert as system row;
        System.runAs(actors[0]) {
            Account visible = [SELECT Id, Name FROM Account WHERE Id=:row.Id WITH USER_MODE];
            System.assertEquals('Original', visible.Name);
            visible.Name = 'Changed';
            update as user visible;
            Account actual = [SELECT Name FROM Account WHERE Id=:row.Id WITH SYSTEM_MODE];
            System.assertEquals('Changed', actual.Name);
        }
    }
}
