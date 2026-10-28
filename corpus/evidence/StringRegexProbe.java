import java.util.*;
public class StringRegexProbe {
  static String q(String x) {return "\""+x.replace("\\","\\\\").replace("\"","\\\"").replace("\n","\\n").replace("\r","\\r")+"\"";}
  public static void main(String[] args) {
    String[][] cases={{"a", "a*"},{"abc", "\\b"},{"a\n","a$"},{"\r","."},{"abb","(?:a)(?U)b+"}};
    for (String[] c:cases) {
      StringJoiner j = new StringJoiner(",", "[", "]");
      for (String s:c[0].split(c[1])) j.add(q(s));
      System.out.println("{\"input\":"+q(c[0])+",\"pattern\":"+q(c[1])+",\"split\":"+j+",\"replaceAll\":"+q(c[0].replaceAll(c[1],"X"))+"}");
    }
  }
}
