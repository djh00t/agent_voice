# Address Book

The address book is stored as a JSON phone book keyed by caller ID.

## Access control entries

The same phone book also controls inbound caller-ID access:

- `*` is the wildcard policy entry for callers that do present caller ID but do not have an exact record.
- `__no_caller_id__` is the policy entry for callers that do not present caller ID.
- Both are created automatically with `disabled: true`, so inbound calls are denied by default unless you enable an exact caller record or relax one of the policy entries.
- Exact caller records are allowed unless `disabled: true`.

## Active-caller-only rule

The assistant may only update the record for the active caller ID on the current call.

If the caller mentions another person, teammate, spouse, or customer, those details must not be written into the active caller record.

## Editable fields

- `first_name`
- `last_name`
- `email`
- `company`
- `timezone`
- `preferred_language`
- `notes`

## Validation rules

- Names must be clearly stated as the caller's own name.
- Email must be read back and confirmed before it is stored.
- Company must clearly belong to the caller.
- Timezone may only be inferred from the caller's own explicit location.
- Preferred language may only be set when the caller explicitly states a preference.
- Notes should stay short, factual, and low priority.

## Email confirmation flow

The service now keeps email writes gated behind a confirmation step:

1. The caller gives an email address.
2. The assistant repeats it back and asks for confirmation.
3. The email is only written after the caller confirms it.

That avoids silently saving a misheard address from telephony STT.

## Anonymous callers

If you explicitly allow `__no_caller_id__`, the assistant can talk to callers without caller ID, but it will not persist profile updates for that call because there is no stable caller record to attach them to.
