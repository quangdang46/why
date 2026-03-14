const DEFAULT_CONTEXT_COMMANDS: &[&str] = &["claude", "sgpt", "llm"];
const MAX_CONTEXT_TARGETS: usize = 5;

pub fn render_shell_functions() -> String {
    render_shell_functions_for(DEFAULT_CONTEXT_COMMANDS)
}

fn render_shell_functions_for(commands: &[&str]) -> String {
    let wrappers = commands
        .iter()
        .map(|command| {
            format!(
                "{command}() {{\n  _why_context_inject_prompt_tool {command} \"$@\"\n}}"
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");

    format!(
        r#"# why context-inject
# eval "$(why context-inject)"
# Wrappers activate for prompt-as-single-argument or piped-stdin flows.

_why_context_inject_targets() {{
  if ! git rev-parse --git-dir >/dev/null 2>&1; then
    return 0
  fi

  local diff_output
  diff_output="$(git diff --cached --unified=0 --diff-filter=ACMR --no-color 2>/dev/null || true)"
  if [ -z "$diff_output" ]; then
    diff_output="$(git diff --unified=0 --diff-filter=ACMR --no-color 2>/dev/null || true)"
  fi
  if [ -z "$diff_output" ]; then
    return 0
  fi

  printf '%s\n' "$diff_output" | awk '
    /^\+\+\+ b\// {{ file = substr($0, 7); next }}
    /^@@ / {{
      if (file != "" && match($0, /\+([0-9]+)/, m)) {{
        print file ":" m[1]
      }}
    }}
  ' | awk '!seen[$0]++' | head -n {MAX_CONTEXT_TARGETS}
}}

_why_context_inject_preamble() {{
  if ! command -v why >/dev/null 2>&1; then
    return 0
  fi

  local combined
  combined="$({
    while IFS= read -r target; do
      [ -n "$target" ] || continue
      output="$(why "$target" --no-llm --json 2>/dev/null || true)"
      [ -n "$output" ] || continue
      printf 'why context for %s:\n%s\n' "$target" "$output"
    done < <(_why_context_inject_targets)
  })"

  [ -n "$combined" ] || return 0
  printf 'Git archaeology context:\n---\n%s---\n\n' "$combined"
}}

_why_context_inject_prompt_tool() {{
  local tool="$1"
  shift
  local preamble
  preamble="$(_why_context_inject_preamble)"

  if [ "$#" -eq 1 ] && [ "${{1#-}}" = "$1" ]; then
    if [ -n "$preamble" ]; then
      command "$tool" "$(printf '%s%s' "$preamble" "$1")"
    else
      command "$tool" "$1"
    fi
    return $?
  fi

  if [ "$#" -gt 0 ]; then
    command "$tool" "$@"
    return $?
  fi

  if [ -t 0 ]; then
    command "$tool"
    return $?
  fi

  if [ -n "$preamble" ]; then
    {{ printf '%s' "$preamble"; cat; }} | command "$tool"
  else
    command "$tool"
  fi
}}

{wrappers}
"#
    )
}

#[cfg(test)]
mod tests {
    use super::render_shell_functions;

    #[test]
    fn shell_output_includes_default_wrappers_and_helpers() {
        let output = render_shell_functions();
        assert!(output.contains("_why_context_inject_targets()"));
        assert!(output.contains("_why_context_inject_preamble()"));
        assert!(output.contains("_why_context_inject_prompt_tool()"));
        assert!(output.contains("claude()"));
        assert!(output.contains("sgpt()"));
        assert!(output.contains("llm()"));
        assert!(output.contains("head -n 5"));
    }

    #[test]
    fn shell_output_prefers_prompt_arg_or_piped_stdin() {
        let output = render_shell_functions();
        assert!(output.contains("if [ \"$#\" -eq 1 ] && [ \"${1#-}\" = \"$1\" ]; then"));
        assert!(output.contains("if [ -t 0 ]; then"));
    }
}
