@IsTest
private class AxtikDeadCodeNegative {
    @IsTest static void sample() {
        if (false) {
            Integer invalid = 'not an integer';
        }
        System.assert(true);
    }
}
