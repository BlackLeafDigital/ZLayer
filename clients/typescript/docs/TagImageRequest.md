
# TagImageRequest

Request body for [`tag_image_handler`]. Matches Docker-compat `docker tag` semantics: create a new reference (`target`) pointing at an already-cached image (`source`).

## Properties

Name | Type
------------ | -------------
`source` | string
`target` | string

## Example

```typescript
import type { TagImageRequest } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "source": null,
  "target": null,
} satisfies TagImageRequest

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as TagImageRequest
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


