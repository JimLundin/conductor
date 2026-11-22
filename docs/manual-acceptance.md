# Conductor hands-on checks

## Claude sign-in

The CLI experiment is complete. For a new setup token, open **Credentials**
in Conductor on the paired phone, enter a **Public account label** under
**Claude setup token**, then tap **Sign in to create a token**. Open Claude's
page and copy its return code. Switch back to Conductor on the same phone,
paste the code, and tap **Send code**. The host stores the setup token.

## Turn snapshots and Continue

Use a prepared session in a disposable project. Ask for a small file
change in each of two turns, then send a turn that changes no files. Open
each turn's diff: the first two should show only their own changes, and
the unchanged turn should remain a separate row with an empty diff. If
the agent made several commits in one turn, its diff should include all
of that turn's changes.

After a prepared host restart, select **Continue** and send a follow-up
about the earlier conversation. It should retain the conversation and
workspace. The original checkout should remain unchanged. Historical
undo is unavailable.

For a session on an issue, select **Finish** and check its landing result.
Remove the stopped session and confirm its workspace disappears while
the session branch and its commits remain in the checkout. Check the
reported push result before continuing the issue on another host.

## Later, when each feature is ready

| Feature | Your part on the iPhone |
| --- | --- |
| Phone sign-in | Approve the disposable Claude or ChatGPT sign-in when prompted. |
| Notifications | In the Home Screen app, enable notifications and lock the phone. Tap the test notification and confirm it opens the session with the pending request. |
| Two hosts | Tap **Add host** for the prepared Mac. Start a new session there, send a message from the phone, then switch back to the original host's session and send a follow-up. |
| Checklists and subagents | Watch the prepared session's checklist update. Open its child sessions to see their progress, requests and results. |

The agent prepares the hosts and sessions, runs the four-day and day-nine checks, and records the results.
