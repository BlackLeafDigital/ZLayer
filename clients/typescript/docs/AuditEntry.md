
# AuditEntry

A recorded audit log entry capturing who did what, when, and to which resource.

## Properties

Name | Type
------------ | -------------
`action` | string
`createdAt` | string
`details` | object
`id` | string
`ip` | string
`resourceId` | string
`resourceKind` | string
`userAgent` | string
`userId` | string

## Example

```typescript
import type { AuditEntry } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "action": null,
  "createdAt": 2026-04-15T12:00:00Z,
  "details": null,
  "id": null,
  "ip": null,
  "resourceId": null,
  "resourceKind": null,
  "userAgent": null,
  "userId": null,
} satisfies AuditEntry

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as AuditEntry
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


