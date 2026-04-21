
# CreateVolumeRequest

Request body for `POST /api/v1/volumes`.

## Properties

Name | Type
------------ | -------------
`labels` | { [key: string]: string; }
`name` | string
`size` | string
`tier` | string

## Example

```typescript
import type { CreateVolumeRequest } from '@zlayer/api-client'

// TODO: Update the object below with actual values
const example = {
  "labels": null,
  "name": null,
  "size": null,
  "tier": null,
} satisfies CreateVolumeRequest

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as CreateVolumeRequest
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


