@IsTest
private class AxtikDeadCodePositive {
    @IsTest static void sample() {
        if (false) {
            Integer valid = 7;
            System.assertEquals(7, valid);
        }
        System.assert(true);
    }
}
