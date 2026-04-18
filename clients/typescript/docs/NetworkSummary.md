
# NetworkSummary

Summary returned when listing networks.

## Properties

Name | Type
------------ | -------------
`cidrCount` | number
`description` | string
`memberCount` | number
`name` | string
`ruleCount` | number

## Example

```typescript
import type { NetworkSummary } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "cidrCount": null,
  "description": null,
  "memberCount": null,
  "name": null,
  "ruleCount": null,
} satisfies NetworkSummary

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as NetworkSummary
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


