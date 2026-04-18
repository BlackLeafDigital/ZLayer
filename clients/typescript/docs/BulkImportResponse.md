
# BulkImportResponse

Result body for `POST /api/v1/secrets/bulk-import`.

## Properties

Name | Type
------------ | -------------
`created` | number
`errors` | Array&lt;string&gt;
`updated` | number

## Example

```typescript
import type { BulkImportResponse } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "created": null,
  "errors": null,
  "updated": null,
} satisfies BulkImportResponse

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as BulkImportResponse
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


