"""What a binary of the build prints, run as the build's own target makes it."""

def _tool_output_impl(ctx):
    out = ctx.actions.declare_file(ctx.attr.out)
    ctx.actions.run_shell(
        outputs = [out],
        tools = [ctx.attr.tool[DefaultInfo].files_to_run],
        command = 'tool=$1 out=$2; shift 2; "$tool" "$@" > "$out"',
        arguments = [ctx.executable.tool.path, out.path] + ctx.attr.arguments,
        mnemonic = "ToolOutput",
        progress_message = "Writing %{output}",
    )
    return [DefaultInfo(files = depset([out]))]

tool_output = rule(
    implementation = _tool_output_impl,
    doc = """Writes what <tool> prints, given <arguments>, to <out>.

A genrule's tools are built in the exec configuration: a zirc binary there is compiled a
second time, with every zirc crate it uses, though the build makes the same binary for its
target anyway. Up to seven minutes of a landing's three Bazel jobs went to that, for
generated files that //controller and //ops only compare with their copies in the tree
(builds 4405 to 4433). Here the tool is the target's own, so the machine building must be
able to run it: use it only for files no other platform's build reads.""",
    attrs = {
        "arguments": attr.string_list(doc = "What the tool is given."),
        "out": attr.string(mandatory = True, doc = "The file it writes, in this package."),
        "tool": attr.label(
            executable = True,
            cfg = "target",
            mandatory = True,
            doc = "A binary the machine building can run.",
        ),
    },
)
