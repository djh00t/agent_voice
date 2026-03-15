# Address Book

The address book is stored as a JSON phone book keyed by caller ID.

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
