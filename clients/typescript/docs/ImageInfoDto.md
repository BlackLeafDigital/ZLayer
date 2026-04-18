
# ImageInfoDto

Serializable wrapper for [`ImageInfo`] so we can attach `ToSchema` here (the underlying type in `zlayer-agent` can\'t depend on `utoipa`).

## Properties

Name | Type
------------ | -------------
`digest` | string
`reference` | string
`sizeBytes` | number

## Example

```typescript
import type { ImageInfoDto } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "digest": null,
  "reference": null,
  "sizeBytes": null,
} satisfies ImageInfoDto

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as ImageInfoDto
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


