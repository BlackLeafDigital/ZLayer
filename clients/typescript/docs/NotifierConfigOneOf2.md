
# NotifierConfigOneOf2

Generic HTTP webhook configuration.

## Properties

Name | Type
------------ | -------------
`headers` | { [key: string]: string; }
`method` | string
`type` | string
`url` | string

## Example

```typescript
import type { NotifierConfigOneOf2 } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "headers": null,
  "method": null,
  "type": null,
  "url": null,
} satisfies NotifierConfigOneOf2

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as NotifierConfigOneOf2
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


