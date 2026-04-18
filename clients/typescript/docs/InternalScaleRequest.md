
# InternalScaleRequest

Request to scale a service

## Properties

Name | Type
------------ | -------------
`replicas` | number
`service` | string

## Example

```typescript
import type { InternalScaleRequest } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "replicas": null,
  "service": null,
} satisfies InternalScaleRequest

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as InternalScaleRequest
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


