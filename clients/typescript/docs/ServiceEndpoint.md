
# ServiceEndpoint

Service endpoint

## Properties

Name | Type
------------ | -------------
`name` | string
`port` | number
`protocol` | string
`url` | string

## Example

```typescript
import type { ServiceEndpoint } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "name": null,
  "port": null,
  "protocol": null,
  "url": null,
} satisfies ServiceEndpoint

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as ServiceEndpoint
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


