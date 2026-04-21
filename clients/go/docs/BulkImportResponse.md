# BulkImportResponse

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Created** | **int32** | Number of new secrets created. | 
**Errors** | **[]string** | Per-line errors. Empty when every line parsed and stored cleanly. | 
**Updated** | **int32** | Number of existing secrets updated. | 

## Methods

### NewBulkImportResponse

`func NewBulkImportResponse(created int32, errors []string, updated int32, ) *BulkImportResponse`

NewBulkImportResponse instantiates a new BulkImportResponse object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewBulkImportResponseWithDefaults

`func NewBulkImportResponseWithDefaults() *BulkImportResponse`

NewBulkImportResponseWithDefaults instantiates a new BulkImportResponse object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetCreated

`func (o *BulkImportResponse) GetCreated() int32`

GetCreated returns the Created field if non-nil, zero value otherwise.

### GetCreatedOk

`func (o *BulkImportResponse) GetCreatedOk() (*int32, bool)`

GetCreatedOk returns a tuple with the Created field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetCreated

`func (o *BulkImportResponse) SetCreated(v int32)`

SetCreated sets Created field to given value.


### GetErrors

`func (o *BulkImportResponse) GetErrors() []string`

GetErrors returns the Errors field if non-nil, zero value otherwise.

### GetErrorsOk

`func (o *BulkImportResponse) GetErrorsOk() (*[]string, bool)`

GetErrorsOk returns a tuple with the Errors field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetErrors

`func (o *BulkImportResponse) SetErrors(v []string)`

SetErrors sets Errors field to given value.


### GetUpdated

`func (o *BulkImportResponse) GetUpdated() int32`

GetUpdated returns the Updated field if non-nil, zero value otherwise.

### GetUpdatedOk

`func (o *BulkImportResponse) GetUpdatedOk() (*int32, bool)`

GetUpdatedOk returns a tuple with the Updated field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetUpdated

`func (o *BulkImportResponse) SetUpdated(v int32)`

SetUpdated sets Updated field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


