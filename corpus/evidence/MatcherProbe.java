import java.util.regex.*;
public class MatcherProbe {
  static String quote(String x) {return "\""+x.replace("\\","\\\\").replace("\"","\\\"").replace("\n","\\n").replace("\r","\\r")+"\"";}
  public static void main(String[] args) {
    String[][] cases = {
      {"a","ba","MFF"},{"a","ba","LFF"},{"a","baba","FMFF"},
      {"a*","ba","FMFFF"},{"^","b","FFFFF"},{"a*","b","FFFFF"},
      {"a|ab","ab","MFF"},{"a|ab","ab","FMFF"},{"a","b","FFFFF"},
      {"a$","a\n","MFF"},{".","\r","MFF"},{"(?m)^a$","a\r","MFF"},{"(?m)^a$","a\n","MFF"}
    };
    for(String[] c:cases){Matcher m=Pattern.compile(c[0]).matcher(c[1]);StringBuilder out=new StringBuilder();
      for(char op:c[2].toCharArray()){boolean yes=op=='F'?m.find():op=='M'?m.matches():m.lookingAt();if(out.length()>0)out.append(',');out.append("{\"op\":").append(quote(""+op)).append(",\"matched\":").append(yes);if(yes)out.append(",\"start\":").append(m.start()).append(",\"end\":").append(m.end()).append(",\"group\":").append(quote(m.group()));out.append('}');}
      System.out.println("{\"pattern\":"+quote(c[0])+",\"input\":"+quote(c[1])+",\"operations\":["+out+"]}");
    }
  }
}
