@IsTest
private class CalculatorTest {
    @IsTest static void exactDecimalArithmetic() {
        System.assertEquals(37.50, Calculator.total(12.50, 3));
        System.assertEquals(0.3, 0.1 + 0.2);
    }
    @IsTest static void evaluatesLoopAndMethod() {
        System.assertEquals(55, Calculator.sumTo(10));
    }
}
