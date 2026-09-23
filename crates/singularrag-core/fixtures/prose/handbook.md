# Northwind handbook

This handbook explains how work gets done at Northwind. It covers the four processes every new colleague meets in their first months. When a rule here disagrees with what your team actually does, tell the People team so one of them can change.

## Onboarding

Your first day starts with the People team, who send the contract, the payroll forms and a schedule for the week. Before you arrive the IT desk prepares your laptop and creates your account in Okta, the single sign-on system that opens every other tool. You activate Okta on the first morning with someone from the IT desk beside you, and from then on you sign in to mail, chat and the code host through it.

Every new colleague is also given a buddy: an experienced person from a neighbouring team who is not your manager. Your buddy has lunch with you in the first week, answers the questions that feel too small to ask anyone else, and checks in once a week for your first two months.

## Expense process

If you spend your own money on work, you get it back through Expensify. Submit each expense in Expensify within a month of paying, with a photo of the receipt and a one-line reason. Your manager approves or rejects the claim, usually within a couple of days. Approved claims then go to Finance, who review them within 5 days and add the amount to your next salary payment. Finance may ask you for a missing receipt, which pauses the clock until you reply.

Travel follows the same path, but book flights and hotels through the travel desk first so the costs are agreed up front.

## Incident process

When a customer-facing system breaks, the on-call engineer is paged through PagerDuty. The on-call engineer acknowledges the page, opens a thread in the incident channel and posts what they know. Everyone who joins the work talks in that channel, not in private messages, so the timeline stays in one place. The on-call engineer leads until the incident is resolved or hands the lead over to someone else, and says so in the channel.

Within 3 days of resolution, whoever led the response writes a post-mortem. It records what happened, when it happened, why it happened and what we will change. Post-mortems are blameless: they describe systems and decisions, never people.

## Release process

Each release has a release captain, chosen in turn from the engineers who contributed to it. The captain decides when the release branch is cut, deploys it to staging, and runs the checks there with the help of anyone who wants to test their own change.

When staging looks healthy, the captain writes the changelog, listing every change that ships with a link to its pull request, and then promotes the build to production. If anything goes wrong after the release, the captain rolls back first and asks questions afterwards.
