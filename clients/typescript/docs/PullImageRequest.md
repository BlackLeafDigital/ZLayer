
# PullImageRequest

Request body for [`pull_image_handler`]. Blocking pull of an OCI image.

## Properties

Name | Type
------------ | -------------
`pullPolicy` | string
`reference` | string

## Example

```typescript
import type { PullImageRequest } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "pullPolicy": null,
  "reference": null,
} satisfies PullImageRequest

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as PullImageRequest
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


