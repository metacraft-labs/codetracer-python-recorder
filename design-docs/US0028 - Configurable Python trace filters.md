---
type: User Story
status: Draft
priority: Critical
persona: "Python team lead"
effort: High
target_release: "Code Review GA"
related_prds:
  - PRD001 - Code Review
related_tasks: []
tags:
  - "#user-story"
  - "#product"
created: 2025-10-11
modified: 2025-10-11
---

# User Story

## Story Statement
As a **Python team lead**, I want **a powerful configuration language to filter which packages, files, code objects, and variables are traced** so that **I can control overhead and focus on relevant code paths**.

## Acceptance Criteria
- [ ] Scenario: Include/exclude by module patterns  
  - Given I provide a configuration that includes `my_app.*` and excludes `my_app.tests.*`  
  - When I run the recorder  
  - Then only functions within the included modules generate events unless explicitly excluded
- [ ] Scenario: Selective variable capture  
  - Given my config specifies locals to include `user`, `order` and exclude `password`  
  - When I inspect a trace event  
  - Then only the allowed variables are serialized, with excluded variables redacted
- [ ] Scenario: Merge multiple filter files  
  - Given I provide a base filter `filters/common.trace` and user-specific overrides `filters/local.trace` combined as `filters/common.trace::filters/local.trace`  
  - When I launch the recorder  
  - Then the merged configuration applies deterministic precedence and validation before tracing starts
- [ ] Scenario: Default filter protects secrets  
  - Given no filter file is provided  
  - When the recorder starts  
  - Then a built-in best-effort secret redaction policy is applied, standard-library/asyncio frames are skipped, and the user is notified how to supply a project-specific filter
- [ ] Scenario: Validate configuration errors  
  - Given I supply an invalid rule (e.g., circular include)  
  - When I launch the recorder  
  - Then a clear validation error points to the problematic rule before tracing starts

## Functional Requirements
- **Scope Filtering**: Model tracing intent as an ordered list of scope rules, each identified by a single selector string that encodes package/module, filesystem path, or fully qualified code object plus match semantics. Selectors later in the list override earlier ones when they overlap. The selector grammar must support globbing by default with opt-in regular expressions while keeping predictable precedence (e.g., object > file > package) so maintainers can quickly isolate code under investigation. Every scope rule defines both an execution capture policy (trace vs skip) and a nested value capture policy processed top-down within the scope.
- **Value Capture Controls**: Within each scope rule, evaluate a top-down list of value patterns (locals, globals, arguments, return payloads, and optionally nested attributes). Patterns resolve to allow-or-deny decisions, with denied values redacted while preserving variable names to indicate omission.
- **I/O Capture Toggle**: Expose a filter flag to enable/disable interceptors for stdout, stderr, stdin, and file descriptors, aligning with the concurrent IO capture effort.
- **Configuration Source**: Filters live in a human-editable file (default path: `<project_root>/.codetracer/trace-filter.cfg`) and can be overridden via CLI/API parameters for alternate locations.
- **Filter Composition**: Support chained composition using the `filter_a::filter_b` syntax, where later filters extend or override earlier ones with clear conflict resolution rules.
- **Default Policy**: Ship a curated default filter that aggressively redacts common secrets (tokens, passwords, keys) and excludes sensitive system modules. This fallback activates when no project filter is found.

## Unified Scope Selector Format
Scope rules and value patterns share a single selector string so that every rule is expressed uniformly. The format is designed to be human writable, unambiguous to parse, and flexible enough for pattern-based matching.

```
<selector> := <kind> ":" [<match_type> ":"] <pattern>
```

- `<kind>` identifies the selector domain. Accepted values depend on the context where the selector appears:
  - **Scope rules (`scope.rules` entries)**:  
    - `pkg` — fully qualified Python packages or modules (`import` dotted names).  
    - `file` — project-relative filesystem paths (POSIX-style separators).  
    - `obj` — fully qualified code objects (functions, classes, methods).
  - **Value patterns (`scope.rules.value_patterns` entries)**:  
    - `local` — local variables within the traced frame.  
    - `global` — module globals referenced by the frame.  
    - `arg` — function arguments by name.  
    - `ret` — return values emitted by the scope (only meaningful for `obj` selectors).  
    - `attr` — attributes on captured values (future friendly for nested fields).
- `<match_type>` is optional; when omitted the default is `glob`. Supported values:
  - `glob` — shell-style wildcards (`*`, `?`, `**` with path semantics for files).
  - `regex` — Python regular expression evaluated with `fullmatch` for deterministic results.
  - `literal` — exact, case-sensitive comparison.
- `<pattern>` is the remaining portion of the string after the optional second colon. Colons inside the pattern are allowed and do not require escaping (they remain part of the final field because parsing stops after two separators). Whitespace is not stripped; leading/trailing spaces must be intentional.

### Selector Examples
- `pkg:my_app.core.*` → package match using default glob semantics.
- `pkg:regex:^my_app\.(services|api)\.` → package match using a regular expression.
- `file:my_app/services/**/*.py` → filesystem glob rooted at the project directory.
- `file:literal:my_app/tests/regression/test_login.py` → exact path match.
- `obj:my_app.auth.secrets.*` → glob match for code objects in the auth secrets namespace.
- `obj:regex:^my_app\.payments\.[A-Z]\w+$` → regex for class names in `payments`.
- `local:literal:user` → value selector targeting the local variable `user`.
- `arg:password` → glob selector (implicit) matching arguments named `password`.
- `ret:regex:^my_app\.auth\.login$` → regex selector applied to fully qualified callable names whose return should be redacted.

### Parsing Prototype
```python
from dataclasses import dataclass
from enum import Enum


class SelectorKind(Enum):
    PACKAGE = "pkg"
    FILE = "file"
    OBJECT = "obj"
    LOCAL = "local"
    GLOBAL = "global"
    ARG = "arg"
    RETURN = "ret"
    ATTR = "attr"


class MatchType(Enum):
    GLOB = "glob"
    REGEX = "regex"
    LITERAL = "literal"


@dataclass(frozen=True)
class Selector:
    kind: SelectorKind
    match_type: MatchType
    pattern: str


class SelectorParseError(ValueError):
    """Raised when a selector string is malformed."""


def parse_selector(raw: str) -> Selector:
    if not raw:
        raise SelectorParseError("selector string is empty")

    parts = raw.split(":", 2)
    if len(parts) < 2:
        raise SelectorParseError(
            f"selector '{raw}' must contain at least a kind and pattern"
        )

    kind_token, remainder = parts[0], parts[1:]
    try:
        kind = SelectorKind(kind_token)
    except ValueError as exc:
        raise SelectorParseError(f"unsupported selector kind '{kind_token}'") from exc

    if len(remainder) == 1:
        match_type = MatchType.GLOB
        pattern = remainder[0]
    else:
        match_token, pattern = remainder
        try:
            match_type = MatchType(match_token)
        except ValueError as exc:
            raise SelectorParseError(
                f"unsupported match type '{match_token}' for selector '{raw}'"
            ) from exc

    if not pattern:
        raise SelectorParseError("selector pattern cannot be empty")

    return Selector(kind=kind, match_type=match_type, pattern=pattern)
```

Callers validate whether a parsed selector is legal in the current context (e.g., scope rules only admit `pkg`, `file`, `obj`; value patterns only admit `local`, `global`, `arg`, `ret`, `attr`).

### Rule Evaluation Order
1. Initialize the execution policy to `scope.default_exec` (or the inherited value when composing filters).  
2. Walk `scope.rules` from top to bottom. Each rule whose selector matches the current frame updates the execution policy (`trace` vs `skip`) and the active default for value capture. Later matching rules replace earlier decisions because the traversal never rewinds.  
3. For value capture inside a scope, start from the applicable default (`scope.default_value_action`, overridden by the scope rule’s `value_default` when provided).  
4. Apply each `value_patterns` entry in order. The first pattern whose selector matches the variable or payload sets the decision to `allow` (serialize), `redact` (replace with `<redacted>`), or `drop` (omit entirely; return-value drops still emit a structural return edge with a `<dropped>` placeholder) and stops further evaluation for that value.  
5. If no pattern matches, fall back to the current default value action.  

## Sample Filters (TOML)
The examples below illustrate the breadth of rules a maintainer can express and how contributors extend the baseline.

```toml
# .codetracer/trace-filter.toml - Maintainer-distributed baseline
[meta]
name = "myapp-maintainer-default"
version = 1
description = "Safe defaults for MyApp support traces."

[io]
capture = false                # Disable IO capture until opted-in explicitly
streams = ["stdout", "stderr"] # Streams to include if `capture` becomes true

[scope]
default_exec = "skip"               # Start from skip-all to avoid surprises
default_value_action = "redact"     # Redact values unless allowed explicitly

[[scope.rules]]
selector = "pkg:my_app.core.*"      # Capture primary business logic
exec = "trace"
value_default = "redact"

[[scope.rules.value_patterns]]
selector = "local:literal:user"
action = "allow"

[[scope.rules.value_patterns]]
selector = "local:literal:order"
action = "allow"

[[scope.rules.value_patterns]]
selector = "arg:password"
action = "redact"

[[scope.rules.value_patterns]]
selector = "global:literal:FEATURE_FLAGS"
action = "allow"

[[scope.rules.value_patterns]]
selector = "attr:regex:(?i).*token"
action = "redact"

[[scope.rules]]
selector = "file:my_app/services/**/*.py" # Allow select service modules by path
exec = "trace"
value_default = "inherit"

[[scope.rules]]
selector = "pkg:my_app.tests.*"     # Skip test suites
exec = "skip"
reason = "Tests generate noise"

[[scope.rules]]
selector = "obj:my_app.auth.secrets.*" # Block sensitive auth helpers entirely
exec = "skip"
reason = "Auth helpers contain secrets"

[[scope.rules]]
selector = "obj:my_app.auth.login"
exec = "trace"
value_default = "inherit"

[[scope.rules.value_patterns]]
selector = "ret:literal:my_app.auth.login"
action = "redact"
reason = "Redact login return payloads"

[[scope.rules]]
selector = "obj:my_app.payments.capture_payment"
exec = "trace"
value_default = "redact"

[[scope.rules.value_patterns]]
selector = "local:literal:invoice"
action = "allow"

[[scope.rules.value_patterns]]
selector = "local:literal:amount"
action = "allow"

[[scope.rules.value_patterns]]
selector = "arg:literal:invoice_id"
action = "allow"

[[scope.rules.value_patterns]]
selector = "arg:literal:trace_id"
action = "allow"

[[scope.rules.value_patterns]]
selector = "local:literal:card_number"
action = "redact"
```

```toml
# ~/.codetracer/local-overrides.toml - Contributor-specific overrides
[meta]
name = "maintainer-default overrides for bug #4821"
version = 1

[scope]
default_exec = "inherit"              # Defer to baseline rules when unspecified
default_value_action = "inherit"

[[scope.rules]]
selector = "file:my_app/tests/regression/test_login.py"
exec = "trace"
value_default = "inherit"
reason = "Capture failing regression suite locally"

[[scope.rules.value_patterns]]
selector = "local:literal:debug_context"
action = "allow"                      # Allow one extra local for this capture

[io]
capture = true
streams = ["stdout"]                  # Only record stdout noise relevant to bug
```

## TOML Schema
The recorder validates filter files against the schema below. Keys not listed are rejected to prevent silent typos.

### Root Tables
- **`meta`** (required table)  
  - `name` *(string, required)*: Human-readable identifier; must be non-empty.  
  - `version` *(integer, required)*: Schema version ≥1 for forward-compat negotiation.  
  - `description` *(string, optional)*: Free-form context for maintainers.  
  - `labels` *(array[string], optional)*: Arbitrary tags; duplicates are ignored.
- **`io`** (optional table)  
  - `capture` *(bool, default `false`)*: Master switch for IO interception.  
  - `streams` *(array[string], optional)*: Subset of `["stdout","stderr","stdin","files"]`; must be present when `capture = true`.  
  - `modes` *(array[string], optional)*: Future expansion for granular IO sources; currently must be empty if provided.
- **`scope`** (required table)  
  - `default_exec` *(string, required)*: One of `trace`, `skip`, `inherit`. `inherit` is only valid when the filter participates in composition.  
  - `default_value_action` *(string, required)*: One of `allow`, `deny`, `inherit`. Defines the baseline decision for value capture before per-scope overrides execute.  
  - `[[scope.rules]]` *(array table, optional)*: Ordered list of scope-specific overrides processed top-to-bottom. Each rule supports:
    - `selector` *(string, required)*: Unified scope selector string (see "Unified Scope Selector Format").  
    - `exec` *(string, optional)*: `trace`, `skip`, or `inherit` (defaults to `inherit`).  
    - `value_default` *(string, optional)*: `allow`, `deny`, or `inherit` (defaults to `inherit`).  
    - `reason` *(string, optional)*: Audit trail explaining the rule’s intent.  
    - `[[scope.rules.value_patterns]]` *(array table, optional)*: Ordered allow/deny decisions for value capture within this scope:
      - `selector` *(string, required)*: Unified selector string targeting value domains (`local`, `global`, `arg`, `ret`, `attr`).  
      - `action` *(string, required)*: Either `allow` or `deny`. `deny` results in redaction.  
      - `reason` *(string, optional)*: Document why the pattern exists.  

### Composition Semantics
- Filters may be combined via `filter_a::filter_b`. Evaluation walks the chain left → right; later filters override earlier ones when keys conflict.
- `inherit` defaults carry the value from the previous filter in the chain; if no prior value exists, validation fails with a descriptive error.
- `scope.rules` arrays merge by appending, so rules contributed by later filters execute after earlier ones and can override them through ordered evaluation.
- Nested `value_patterns` arrays also append, preserving the expectation that later entries refine or replace earlier decisions.

## Notes & Context
- **Problem / Opportunity**: Teams need precise control to manage performance, privacy, and noise.
- **Assumptions**: Configuration supports hierarchical scopes, globbing, and precedence rules.
- **Primary Use Case**: Maintainership workflows where project owners publish a vetted filter file and instruct contributors to record traces for bug reports without exposing unrelated or sensitive code paths.
- **Safety Goals**: Default and project-authored filters should minimize the risk of leaking credentials, PII, or third-party secrets while keeping signals required for debugging.
- **Design References**: Planned DSL/reference doc plus UI for editing rules.

## Metrics & Impact
- **Primary Metric**: ≥50% of Python projects adopt custom filters within first month of availability.
- **Guardrails**: Config parsing executes in <200ms and tracing overhead ≤10% when filters are active.

## Dependencies
- **Technical**: Config parser and evaluator; runtime hooks to enforce include/exclude at event time.
- **Cross-Team**: Security review of default redaction list; Docs for config reference and examples.

## Links
- **Related Tasks**: 
- **Design Artifacts**: 

```dataview
TABLE status, due_date, priority
FROM "10-Tasks"
WHERE contains(this.file.related_tasks, file.name)
```

```dataview
TABLE status, milestone, priority
FROM ""
WHERE contains(this.file.related_prds, file.name)
```

```dataview
TABLE status, milestone, priority
FROM ""
WHERE file.frontmatter.type = "PRD" AND contains(file.frontmatter.related_stories, this.file.name)
```

## Open Questions
- [ ] Do we need UI tooling for config authoring or is CLI/editor workflow sufficient for GA?

## Next Step
- [ ] Define grammar and precedence rules for the tracing configuration language.
