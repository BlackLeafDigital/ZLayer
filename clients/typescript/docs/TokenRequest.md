
# TokenRequest

Token request

## Properties

Name | Type
------------ | -------------
`apiKey` | string
`apiSecret` | string

## Example

```typescript
import type { TokenRequest } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "apiKey": null,
  "apiSecret": null,
} satisfies TokenRequest

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as TokenRequest
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


