
# PullImageResponse

Response body for [`pull_image_handler`]. Reports the pulled reference and, when the backend exposes it via `list_images`, the resolved digest and on-disk size.

## Properties

Name | Type
------------ | -------------
`digest` | string
`reference` | string
`sizeBytes` | number

## Example

```typescript
import type { PullImageResponse } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "digest": null,
  "reference": null,
  "sizeBytes": null,
} satisfies PullImageResponse

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as PullImageResponse
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


