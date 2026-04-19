
# StoredNotifier

A stored notifier — a named notification channel that fires alerts to Slack, Discord, a generic webhook, or SMTP when triggered.

## Properties

Name | Type
------------ | -------------
`config` | [NotifierConfig](NotifierConfig.md)
`createdAt` | string
`enabled` | boolean
`id` | string
`kind` | [NotifierKind](NotifierKind.md)
`name` | string
`updatedAt` | string

## Example

```typescript
import type { StoredNotifier } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "config": null,
  "createdAt": 2026-04-15T12:00:00Z,
  "enabled": null,
  "id": null,
  "kind": null,
  "name": null,
  "updatedAt": 2026-04-15T12:00:00Z,
} satisfies StoredNotifier

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as StoredNotifier
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


