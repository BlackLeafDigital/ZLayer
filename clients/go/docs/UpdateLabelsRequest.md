# UpdateLabelsRequest

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Labels** | **map[string]string** | Labels to add or update | 
**Remove** | **[]string** | Label keys to remove | 

## Methods

### NewUpdateLabelsRequest

`func NewUpdateLabelsRequest(labels map[string]string, remove []string, ) *UpdateLabelsRequest`

NewUpdateLabelsRequest instantiates a new UpdateLabelsRequest object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewUpdateLabelsRequestWithDefaults

`func NewUpdateLabelsRequestWithDefaults() *UpdateLabelsRequest`

NewUpdateLabelsRequestWithDefaults instantiates a new UpdateLabelsRequest object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetLabels

`func (o *UpdateLabelsRequest) GetLabels() map[string]string`

GetLabels returns the Labels field if non-nil, zero value otherwise.

### GetLabelsOk

`func (o *UpdateLabelsRequest) GetLabelsOk() (*map[string]string, bool)`

GetLabelsOk returns a tuple with the Labels field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetLabels

`func (o *UpdateLabelsRequest) SetLabels(v map[string]string)`

SetLabels sets Labels field to given value.


### GetRemove

`func (o *UpdateLabelsRequest) GetRemove() []string`

GetRemove returns the Remove field if non-nil, zero value otherwise.

### GetRemoveOk

`func (o *UpdateLabelsRequest) GetRemoveOk() (*[]string, bool)`

GetRemoveOk returns a tuple with the Remove field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetRemove

`func (o *UpdateLabelsRequest) SetRemove(v []string)`

SetRemove sets Remove field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


