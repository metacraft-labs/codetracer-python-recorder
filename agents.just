consolidate start_change_id end_change_id:
	#!/usr/bin/env sh
	read -r -d '' INSTRUCTIONS <<-EOF
	You will be given a diff with 'jj diff --from {{start_change_id}} --to {{end_change_id}}'. The task is to write a specification which describes the introduced changes.

	You need to produce a full detailed specification in the folder 'specs/'. Imagine that the changes that the diff shows disappeared and we had to implement the functionality from scratch. The specification must contain all necessary details in order to be ample to implement equivalent functionality.

	Abide by the following rules:

	`cat rules/spec.md`

	Here is a log of the changes in question:

	`jj log -r "({{start_change_id}}::{{end_change_id}}) ~ {{start_change_id}}" -T builtin_log_detailed`


	Here is the diff in question:

	`jj diff --from {{start_change_id}} --to {{end_change_id}}`

	EOF
	
	jj new -r {{start_change_id}} -m "Consolidation in progress"	
	
	echo "$INSTRUCTIONS" | codex exec --full-auto --config model_reasoning_effort=high

questions-for-pm rev='@':
	#!/usr/bin/env sh
	CURRENT_CHANGE=`jj log -r @ --template 'change_id' --no-graph`

	read -r -d '' INSTRUCTIONS_1 <<-EOF
	    1. Read the current state of open issues in issues.md
		2. Read relevant state of the code base
		3. Read 'pm-faq.md' which contains previous questions asked to the product manager and her answers.
		4. Create questions to the product manager to clarify unclear design choices, priorities and so on. If everything is clear and no questions are needed do nothing.
		5. Append the new questions to the end of 'pm-faq.md' file
	EOF

	jj new -r {{rev}} -m "Questions for the PM"
	PM_CHANGE=`jj log -r @ --template 'change_id' --no-graph`

	# Add a dated header once
	echo "# Questions from `date -I`" >> pm-faq.md

	# Initialize iteration counter for commit messages
	ITERATION=1

	# Keep asking until no further changes are made to pm-faq.md
	while true; do
		PREV_HASH=`[ -f pm-faq.md ] && sha256sum pm-faq.md | cut -d' ' -f1 || echo ""`

		echo "$INSTRUCTIONS_1" | codex exec --full-auto --config model_reasoning_effort=high

		CURR_HASH=`[ -f pm-faq.md ] && sha256sum pm-faq.md | cut -d' ' -f1 || echo ""`
		[ "$CURR_HASH" = "$PREV_HASH" ] && break

		# Optional manual edits
		jj edit $CURRENT_CHANGE && just agents::edit $PM_CHANGE
		# Update issues based on PM responses
		just agents::pm-flow-update $PM_CHANGE

		# Start a new iteration change, include a counter in the message
		jj new -r @ -m "Questions for the PM (iteration ${ITERATION})"
		PM_CHANGE=`jj log -r @ --template 'change_id' --no-graph`
		ITERATION=$((ITERATION + 1))
	done

	# Return to original change
	jj edit $CURRENT_CHANGE


pm-flow-update rev='@':
	#!/usr/bin/env sh
	read -r -d '' INSTRUCTIONS_2 <<-EOF
	    1. Read the current state of open issues in 'issues.md'
		2. Read the conversation with the product manager so far in 'pm-faq.md'
		3. If necessary read relevant parts of the code
		4. Update the issues database in 'issues.md' to match the instructions provided by the product manager.
	EOF

	CURRENT_CHANGE=`jj log -r @ --template 'change_id' --no-graph`

	jj new -r {{rev}} -m "Updating Issue database"
	UPDATE_CHANGE=`jj log -r @ --template 'change_id' --no-graph`

	echo "$INSTRUCTIONS_2" | codex exec --full-auto --config model_reasoning_effort=high


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

	`cat rules/writing.md`

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
	   - DO NOT review code which is unrelated to the change under review. You can review code not in the diff, if its functionality was indirectly changed by the diff under review.
	   - DO NOT write positive review comments. You can write positive statements in the review summary. Use review comments ONLY to point out potential problems or improvements.
   	   
	4. Examine the changes and update the list of issues in 'issues.md'
	   - If the change affects a specific issue update its status
	   - If the change partially solves an issue update its status and create new issues for the part which is not yet solved
	   - If the change affects any non-related issue or creates new issues write that down as well
	   - If you find any problems in the code unrelated to the change under review DO NOT write/update issues about them. We only care about how the change under review affects the overall state of the code and issues.

	5. Update the change description with a summary of the review using 'jj describe --stdin <<REVIEW
	review(codex): ...

	...
	REVIEW'

	`cat rules/writing.md`

	`cat rules/issues.md`
	EOF

tidy-issues:
	#!/usr/bin/env sh
	codex exec --full-auto --config model_reasoning_effort=high <<EOF
	1. Read the file '/issues.md'.
	2. Check if it follows the rules listed below
	3. run 'jj new -m "issues(codex): ..."'. For example 'jj new -m "issues(codex): Removing duplicate dog-barking issue"'
	4. Refactor the issues database to follow the specified rules. Do the MINIMAL changes necessary to make sure that the database follows the rules, while not removing any existing information in it.
	5. Finally update the file '/issues-overview.md' to match the changes in 'issues.md'. It is a table of issues and statuses.

	`cat rules/writing.md`

	`cat rules/issues.md`
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

edit rev='@':
	#!/usr/bin/env sh

	CURRENT_CHANGE=`jj log -r @ --template 'change_id' --no-graph`
	jj edit {{rev}}
	
	jj show -r @ --git | sed -n 's/^diff --git a\/\([^ ]*\) b\/.*/\1/p' | sort -u | xargs -r ${EDITOR:-vi}

	jj edit $CURRENT_CHANGE
	
human-work-step:
	#!/usr/bin/env sh
	just agents::edit
	jj split issues.md -m "issues: Update after review"
	jj edit @-

ai-work-step: next-issue review-change

# Use this step when starting to work on a new issue
work:
	#!/usr/bin/env sh
	while true; do
	  just agents::pick-next-issue
	  [ -f next-issue.md ] || break
	  just agents::ai-work-step
	  just agents::human-work-step
	done

# This should be started if you stop at editor phase and want to continue from where you left off
# Current commit should be the last review commit
continue-work:
	#!/usr/bin/env sh
	while true; do
	  just agents::human-work-step
	  just agents::pick-next-issue
	  [ -f next-issue.md ] || break
	  just agents::ai-work-step
	done
	
