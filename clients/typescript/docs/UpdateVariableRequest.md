
# UpdateVariableRequest

Body for `PATCH /api/v1/variables/{id}`. All fields are optional.

## Properties

Name | Type
------------ | -------------
`name` | string
`value` | string

## Example

```typescript
import type { UpdateVariableRequest } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "name": null,
  "value": null,
} satisfies UpdateVariableRequest

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as UpdateVariableRequest
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


