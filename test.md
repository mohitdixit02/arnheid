## Current Issue
1. Agent is not able to send email. As per logs:
- The agent IS keeping prior reasoning — it references "previous searches did not return any results" in turn 3 & 4, so it does see prior turns.
- The real bug: it keeps calling gsuite_gmail_search instead of gsuite_gmail_send (or similar send tool). It's stuck in a loop checking if the email was sent — but never actually sending it. This is a prompt issue.

2. For the statement "@arnheidgenbot have you sent the email yet? if not sent it now please", it is just ingesting and not doing anything, due to hard coded ingestion.

## Fixes
1. Check for gmail_send_tool expose (currently implemented but need to be verify). Also verify that whether all tools are properly exposed and accessible to the agent. If not, fix the exposure issue. Required tools:
- gsuite_gmail_send
- gsuite_gmail_search
- calendar_event_create
- calendar_event_search
- drive_search
2. For hard coded ingestion, replace it with LLM based router. a light LLM will be given prompt and question and it will clasify whether it is a ingestion or a query or another type of request as per code.



