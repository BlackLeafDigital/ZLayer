
# ContainerEvent

A container lifecycle event published on the bus.

## Properties

Name | Type
------------ | -------------
`at` | Date
`exitCode` | number
`id` | string
`kind` | [ContainerEventKind](ContainerEventKind.md)
`labels` | { [key: string]: string; }
`reason` | string
`status` | string

## Example

```typescript
import type { ContainerEvent } from '@zlayer/api-client'

// TODO: Update the object below with actual values
const example = {
  "at": null,
  "exitCode": null,
  "id": null,
  "kind": null,
  "labels": null,
  "reason": null,
  "status": null,
} satisfies ContainerEvent

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as ContainerEvent
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


