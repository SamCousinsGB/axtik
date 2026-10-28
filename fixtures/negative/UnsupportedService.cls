@IsTest private class UnsupportedService {
    @IsTest static void blocksUnknownPlatformService() {
        Messaging.SingleEmailMessage mail = new Messaging.SingleEmailMessage();
        Messaging.sendEmail(new Messaging.SingleEmailMessage[] { mail });
    }
}
