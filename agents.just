next-issue:
	#!/usr/bin/env sh
	[ -f next-issue.md ] || exit 1

	codex exec --full-auto --config model_reasoning_effort=high <<EOF
	1. You will next work on issue `cat next-issue.md`. Read the actual issue from 'issues.md'
	3. Decide is it relevant to write a failing test for the issue. If so, write  the test, make sure that it fails.
	   Note! Not all issues require writing failing tests. Use discretion. Remember - code is debt.
	4. Change the code to fix the issue. Use "just dev test" to check if changes compile and tests pass.
	   Code must follow the rules in '/rules/source-code.md'
	5. When the issue is fixed and tests pass run "jj describe --stdin <<ENDDESC
	commit message
	ENDDESC".

	For example:
	jj describe --stdin <<ENDDESC
	ISSUE-043: Update dog-barking protocol

	...
	ENDDESC

	Note! If you try to add a commit message like 'jj new -m "blah"' or 'jj describe -m "blah"' the resulting message will not be formatted properly.

	EOF

	rm next-issue.md

review-change:
	#!/usr/bin/env sh
	codex exec --full-auto --config model_reasoning_effort=high <<EOF
	1. Fetch the current changes by writing 'jj show'
	2. Create a new change by calling 'jj new -m "review(codex): ..."'. For example 'jj new -m "review(codex): Issue 004 - introducing dog barking functionality"'
	3. Do a deep review of the change.
	   - Write your review inline in the code itself by using code comments starting with '//REVIEW(author)' (or '# REVIEW(author)') to record your review.
	   - While reviewing make sure that the rules in '/rules' folder are satisfied by the change under review.
	   - DO NOT make any other changes in the code except adding the above-mentioned review comments.
	   - DO NOT review code which is unrelated to the change under review
   	   
	4. Examine the changes and update the list of issues in 'issues.md'
	   - If the change affects a specific issue update its status
	   - If the change partially solves an issue update its status and create new issues for the part which is not yet solved
	   - If the change affects any non-related issue or creates new issues write that down as well
	   - If you find any problems in the code unrelated to the change under review DO NOT write/update issues about them. We only care about how the change under review affects the overall state of the code and issues.

	   Issues file must follow the rules in '/rules/issues.md'
	5. Update the change description with a summary of the review using 'jj describe --stdin <<REVIEW
	review(codex): ...

	...
	REVIEW'
	EOF

tidy-issues:
	#!/usr/bin/env sh
	codex exec --full-auto --config model_reasoning_effort=high <<EOF
	1. Read the file '/issues.md'.
	2. Check if it follows the rules in '/rules/issues.md'
	3. run 'jj new -m "issues(codex): ..."'. For example 'jj new -m "issues(codex): Removing duplicate dog-barking issue"'
	4. Refactor the issues database to follow the specified rules. Do the MINIMAL changes necessary to make sure that the database follows the rules, while not removing any existing information in it.
	5. Finally update the file '/issues-overview.md' to match the changes in 'issues.md'. It is a table of issues and statuses.
	EOF

archive-issues:
	#!/usr/bin/env sh
	codex exec --full-auto --config model_reasoning_effort=high <<EOF
	This is a task to archive done issues.
	1. Read file '/issues.md'.
	2. Check if there are any issues which are DONE. If not, exit.
	3. Run 'jj new -m "issues(codex): Archiving done issues"'
	4. Change the status of all "Done" issues to "Archived"
	5. Then move achived issues to '.archive/issues-yyyy-mm-dd.md' file. Use current date.
	6. Finally update the file '/issues-overview.md'. It is a table of issues and statuses

pick-next-issue:
	#!/usr/bin/env sh
	codex exec --full-auto --config model_reasoning_effort=high <<EOF
	1. Examine issues.md and choose a single issue to work on.
	2. If no issue is chosen, then STOP and don''t do anything else.
	3. Otherwise:
	    3.1. Run "jj new" DON''T add a commit message!
	    3.2. Create a file in the root folder 'next-issue.md' The file must contain ONLY the issue id then a newline and nothing else.
	    3.3. STOP.
	EOF

edit:
	#!/usr/bin/env sh
	jj show -r @ --git | sed -n 's/^diff --git a\/\([^ ]*\) b\/.*/\1/p' | sort -u | xargs -r ${EDITOR:-vi}

work:
	#!/usr/bin/env sh
	echo "Hi"
	while true; do
	  echo "Bye"
	  just agents::pick-next-issue
	  [ -f next-issue.md ] || break
	  just agents::next-issue
	  just agents::review-change
	  EDITOR=emacsclient just agents::edit
	  jj squash issues.md
	  jj edit -r @-
	done
