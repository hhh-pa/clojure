import org.objectweb.asm.*;
import java.nio.file.*;
public class GenModes {
    static void gen(String name, boolean doEnter) throws Exception {
        ClassWriter cw = new ClassWriter(ClassWriter.COMPUTE_FRAMES);
        cw.visit(Opcodes.V17, Opcodes.ACC_PUBLIC, name, null, "java/lang/Object", null);
        MethodVisitor mv = cw.visitMethod(Opcodes.ACC_PUBLIC | Opcodes.ACC_STATIC,
            "run", "()Ljava/lang/String;", null, null);
        mv.visitCode();
        if (doEnter) {
            mv.visitTypeInsn(Opcodes.NEW, "java/lang/Object");
            mv.visitInsn(Opcodes.DUP);
            mv.visitMethodInsn(Opcodes.INVOKESPECIAL, "java/lang/Object", "<init>", "()V", false);
            mv.visitVarInsn(Opcodes.ASTORE, 0);
            mv.visitVarInsn(Opcodes.ALOAD, 0);
            mv.visitInsn(Opcodes.MONITORENTER);
        }
        mv.visitInsn(Opcodes.ACONST_NULL);
        mv.visitInsn(Opcodes.MONITOREXIT);
        mv.visitLdcInsn("no-throw");
        mv.visitInsn(Opcodes.ARETURN);
        mv.visitMaxs(2, 1);
        mv.visitEnd();
        cw.visitEnd();
        Files.write(Paths.get(name + ".class"), cw.toByteArray());
    }
    public static void main(String[] a) throws Exception {
        gen("ExitNilHeld", true);
        gen("ExitNilEmpty", false);
        System.out.println("generated");
    }
}
