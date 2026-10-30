public class RunModes {
    static void one(String cls) {
        try {
            Object r = Class.forName(cls).getMethod("run").invoke(null);
            System.out.println(cls + " -> " + r);
        } catch (Exception e) {
            Throwable t = e.getCause() != null ? e.getCause() : e;
            System.out.println(cls + " THROW " + t.getClass().getName() + " M=" + t.getMessage());
        }
    }
    public static void main(String[] a) { one("ExitNilHeld"); one("ExitNilEmpty"); }
}
