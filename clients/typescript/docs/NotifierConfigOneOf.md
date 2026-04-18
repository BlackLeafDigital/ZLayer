
# NotifierConfigOneOf

Slack incoming webhook configuration.

## Properties

Name | Type
------------ | -------------
`type` | string
`webhookUrl` | string

## Example

```typescript
import type { NotifierConfigOneOf } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "type": null,
  "webhookUrl": null,
} satisfies NotifierConfigOneOf

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as NotifierConfigOneOf
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


