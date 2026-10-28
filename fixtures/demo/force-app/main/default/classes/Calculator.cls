public class Calculator {
    public static Decimal total(Decimal price, Integer quantity) {
        return price * quantity;
    }
    public static Integer sumTo(Integer limit) {
        Integer total = 0;
        for (Integer i = 1; i <= limit; i++) {
            total += i;
        }
        return total;
    }
}
